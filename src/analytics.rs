//! The figures of the dashboard, the maps and the entry statistics.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use tantivy::query::{BooleanQuery, Query};
use tantivy::Searcher;

use crate::collectors::{
    top_terms, CityLabels, Dashboard, DashboardCollector, Layout, MinuteCollector, NumCounts, StatsCollector,
    TermCounts,
};
use crate::query::{self, Filter};
use crate::rollup::{term_hash, GroupRollup, IdMap, IdSet};
use crate::schema::Fields;

/// Browsers, systems and device types on the dashboard.
const TOP_GROUP: usize = 50;
/// URLs on the dashboard.
const TOP_URLS: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum AnalyticsError {
    #[error("time values cannot be negative")]
    NegativeTime,
    #[error("start time must be before end time")]
    InvalidRange,
    #[error("{0}")]
    Index(#[from] tantivy::TantivyError),
}

/// Checks the time range of a request. Zero means open.
pub fn validate_range(start: i64, end: i64) -> Result<(), AnalyticsError> {
    if start < 0 || end < 0 {
        return Err(AnalyticsError::NegativeTime);
    }
    if start > 0 && end > 0 && start >= end {
        return Err(AnalyticsError::InvalidRange);
    }
    Ok(())
}

/// The filter of a geo or statistics request: a log group and a time range in
/// which zero means open.
pub fn range_filter(group: &str, start: i64, end: i64) -> Filter {
    Filter {
        groups: if group.is_empty() { Vec::new() } else { vec![group.to_owned()] },
        start: (start > 0).then_some(start),
        end: (end > 0).then_some(end),
        ..Default::default()
    }
}

// -------------------------------------------------------------- dashboard

#[derive(Debug, Serialize, PartialEq)]
pub struct HourlyStats {
    pub hour: i64,
    pub uv: usize,
    pub pv: u64,
    pub timestamp: i64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct DailyStats {
    pub date: String,
    pub uv: usize,
    pub pv: u64,
    pub timestamp: i64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct UrlStats {
    pub url: String,
    pub visits: u64,
    pub percent: f64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct BrowserStats {
    pub browser: String,
    pub count: u64,
    pub percent: f64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct OsStats {
    pub os: String,
    pub count: u64,
    pub percent: f64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct DeviceStats {
    pub device: String,
    pub count: u64,
    pub percent: f64,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct DashboardSummary {
    pub total_uv: usize,
    pub total_pv: u64,
    pub total_traffic: u64,
    pub avg_daily_uv: f64,
    pub avg_daily_pv: f64,
    pub peak_hour: i64,
    pub peak_hour_traffic: u64,
    pub avg_qps: f64,
    pub peak_qps: f64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct DashboardResponse {
    pub hourly_stats: Vec<HourlyStats>,
    pub daily_stats: Vec<DailyStats>,
    pub top_urls: Vec<UrlStats>,
    pub browsers: Vec<BrowserStats>,
    pub operating_systems: Vec<OsStats>,
    pub devices: Vec<DeviceStats>,
    pub summary: DashboardSummary,
}

fn percent(count: u64, total: u64) -> f64 {
    count as f64 / total as f64 * 100.0
}

/// The figures a dashboard response is made of, however they were gathered.
#[derive(Default)]
pub struct Figures {
    pub window_pv: u64,
    pub window_bytes: u64,
    pub window_uv: usize,
    pub peak_minute: u32,
    pub hourly_pv: Vec<u64>,
    pub hourly_uv: Vec<usize>,
    pub daily_pv: Vec<u64>,
    pub daily_uv: Vec<usize>,
    /// Top browsers, systems, device types and paths, busiest first.
    pub top: [Vec<(String, u64)>; 4],
}

impl From<Dashboard> for Figures {
    fn from(scan: Dashboard) -> Self {
        let limits = [TOP_GROUP, TOP_GROUP, TOP_GROUP, TOP_URLS];
        Figures {
            window_pv: scan.window_pv,
            window_bytes: scan.window_bytes,
            window_uv: scan.window_ips.len(),
            peak_minute: scan.peak_minute(),
            hourly_pv: scan.hourly_pv,
            hourly_uv: scan.hourly_ips.iter().map(|s| s.len()).collect(),
            daily_pv: scan.daily_pv,
            daily_uv: scan.daily_ips.iter().map(|s| s.len()).collect(),
            top: std::array::from_fn(|i| top_terms(&scan.groups[i], limits[i])),
        }
    }
}

/// Assembles the dashboard response from the figures of a scan.
pub fn dashboard_response(layout: &Layout, scan: &Figures) -> DashboardResponse {
    let total_pv = scan.window_pv;
    let mut response = DashboardResponse {
        hourly_stats: Vec::new(),
        daily_stats: Vec::new(),
        top_urls: Vec::new(),
        browsers: Vec::new(),
        operating_systems: Vec::new(),
        devices: Vec::new(),
        summary: DashboardSummary::default(),
    };

    if total_pv > 0 {
        response.hourly_stats = (0..layout.hour_count)
            .map(|i| {
                let stamp = layout.hour_start + i as i64 * 3600;
                HourlyStats {
                    hour: stamp.rem_euclid(86400) / 3600,
                    uv: scan.hourly_uv[i],
                    pv: scan.hourly_pv[i],
                    timestamp: stamp,
                }
            })
            .collect();
        response.daily_stats = layout
            .day_labels
            .iter()
            .enumerate()
            .map(|(i, (date, stamp))| DailyStats {
                date: date.clone(),
                uv: scan.daily_uv[i],
                pv: scan.daily_pv[i],
                timestamp: *stamp,
            })
            .collect();
        response.daily_stats.sort_by_key(|d| d.timestamp);
        let [browsers, systems, devices, urls] = &scan.top;
        response.browsers = browsers
            .iter()
            .map(|(browser, count)| BrowserStats {
                percent: percent(*count, total_pv),
                browser: browser.clone(),
                count: *count,
            })
            .collect();
        response.operating_systems = systems
            .iter()
            .map(|(os, count)| OsStats { percent: percent(*count, total_pv), os: os.clone(), count: *count })
            .collect();
        response.devices = devices
            .iter()
            .map(|(device, count)| DeviceStats {
                percent: percent(*count, total_pv),
                device: device.clone(),
                count: *count,
            })
            .collect();
        response.top_urls = urls
            .iter()
            .map(|(url, visits)| UrlStats { percent: percent(*visits, total_pv), url: url.clone(), visits: *visits })
            .collect();
    }

    let days = response.daily_stats.len();
    let total_uv = scan.window_uv;
    let (mut peak_hour, mut peak_hour_traffic) = (0, 0u64);
    for h in &response.hourly_stats {
        if h.pv > peak_hour_traffic {
            peak_hour = h.hour;
            peak_hour_traffic = h.pv;
        }
    }
    let range = layout.end - layout.start;
    response.summary = DashboardSummary {
        total_uv,
        total_pv,
        total_traffic: scan.window_bytes,
        avg_daily_uv: if days > 0 { total_uv as f64 / days as f64 } else { 0.0 },
        avg_daily_pv: if days > 0 {
            response.daily_stats.iter().map(|d| d.pv).sum::<u64>() as f64 / days as f64
        } else {
            0.0
        },
        peak_hour,
        peak_hour_traffic,
        avg_qps: if range > 0 { total_pv as f64 / range as f64 } else { 0.0 },
        peak_qps: f64::from(scan.peak_minute) / 60.0,
    };
    response
}

/// Runs the dashboard scan of a log group over `[start, end)`.
pub fn dashboard(
    searcher: &Searcher,
    fields: &Fields,
    group: &str,
    start: i64,
    end: i64,
) -> Result<DashboardResponse, AnalyticsError> {
    validate_range(start, end)?;
    let layout = Layout::new(start, end);
    let (lo, hi) = layout.scan_range();
    let filter = Filter {
        groups: if group.is_empty() { Vec::new() } else { vec![group.to_owned()] },
        start: Some(lo),
        end: Some(hi),
        ..Default::default()
    };
    let q = query::build(fields, &filter);
    let scan = searcher.search(q.as_ref(), &DashboardCollector { layout: layout.clone() })?;
    Ok(dashboard_response(&layout, &Figures::from(scan)))
}

/// Whole hours come from the rollups of the groups, the hours that a boundary
/// cuts come from the index. The figures equal those of [`dashboard`].
///
/// A boundary is the start or end of the window, of the scanned range or of a
/// local day. An hour is whole when it lies in the scanned range and no
/// boundary falls inside it. The minutes of the peak come from the rollups
/// when the window starts on a whole minute, otherwise from one pass over the
/// timestamps of the window.
pub fn dashboard_rollup(
    searcher: &Searcher,
    fields: &Fields,
    group: &str,
    start: i64,
    end: i64,
    rollups: &[Arc<GroupRollup>],
) -> Result<DashboardResponse, AnalyticsError> {
    validate_range(start, end)?;
    if start <= 0 || end <= start {
        return dashboard(searcher, fields, group, start, end);
    }
    let layout = Layout::new(start, end);
    let (lo, hi) = layout.scan_range();
    let aligned = start % 60 == 0;

    let mut cuts: Vec<i64> = vec![start, end, lo, hi];
    for (day_lo, day_hi) in &layout.days {
        cuts.push(*day_lo);
        cuts.push(*day_hi);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let is_whole = |h: i64| {
        let next = cuts.partition_point(|c| *c <= h);
        h >= lo && h + 3600 <= hi && cuts.get(next).is_none_or(|c| *c >= h + 3600)
    };

    let mut whole: Vec<i64> = Vec::new();
    let mut edges: Vec<Box<dyn Query>> = Vec::new();
    let mut hour = crate::rollup::hour_of(lo);
    while hour < hi {
        if is_whole(hour) {
            whole.push(hour);
        } else if let Some(range) = query::time_range(fields, Some(hour.max(lo)), Some((hour + 3600).min(hi))) {
            edges.push(range);
        }
        hour += 3600;
    }

    let group_filter =
        Filter { groups: if group.is_empty() { Vec::new() } else { vec![group.to_owned()] }, ..Default::default() };
    let edge = if edges.is_empty() {
        Dashboard::default()
    } else {
        let q =
            BooleanQuery::intersection(vec![query::build(fields, &group_filter), Box::new(BooleanQuery::union(edges))]);
        searcher.search(&q, &DashboardCollector { layout: layout.clone() })?
    };

    let mut minutes = vec![0u32; layout.minute_count];
    let mut window_ips = IdSet::default();
    let mut window_pv = 0u64;
    let mut window_bytes = 0u64;
    let mut hourly_pv = vec![0u64; layout.hour_count];
    let mut hourly_sources: Vec<Vec<&[u64]>> = vec![Vec::new(); layout.hour_count];
    let mut daily_pv = vec![0u64; layout.days.len()];
    let mut daily_ips: Vec<IdSet> = vec![IdSet::default(); layout.days.len()];
    let mut counts: [IdMap<u64>; 4] = Default::default();
    let mut edge_names: [IdMap<&str>; 4] = Default::default();

    let day_of = |ts: i64| {
        let i = layout.days.partition_point(|d| d.0 <= ts).checked_sub(1)?;
        (ts < layout.days[i].1).then_some(i)
    };
    for &h in &whole {
        let in_window = h >= start && h + 3600 <= end;
        let bucket = h - layout.hour_start;
        let bucket = (bucket >= 0 && bucket / 3600 < layout.hour_count as i64).then_some((bucket / 3600) as usize);
        let day = day_of(h);
        for r in rollups {
            let Some(agg) = r.hours.get(&h) else { continue };
            if let Some(b) = bucket {
                hourly_pv[b] += agg.pv;
                hourly_sources[b].push(&agg.ips);
            }
            if let Some(d) = day {
                daily_pv[d] += agg.pv;
                daily_ips[d].extend(agg.ips.iter().copied());
            }
            if in_window {
                window_pv += agg.pv;
                window_bytes += agg.bytes;
                window_ips.extend(agg.ips.iter().copied());
                if aligned {
                    let base = ((h - start) / 60) as usize;
                    for (m, c) in agg.minutes.iter().enumerate() {
                        minutes[base + m] += *c;
                    }
                }
                for (kind, terms) in agg.terms.iter().enumerate() {
                    for (hash, c) in terms {
                        *counts[kind].entry(*hash).or_insert(0) += u64::from(*c);
                    }
                }
            }
        }
    }

    // The hours the boundaries cut
    window_pv += edge.window_pv;
    window_bytes += edge.window_bytes;
    window_ips.extend(edge.window_ips.iter().copied());
    for (a, b) in hourly_pv.iter_mut().zip(&edge.hourly_pv) {
        *a += *b;
    }
    for (a, b) in daily_pv.iter_mut().zip(&edge.daily_pv) {
        *a += *b;
    }
    for (a, b) in daily_ips.iter_mut().zip(&edge.daily_ips) {
        a.extend(b.iter().copied());
    }
    for (kind, terms) in edge.groups.iter().enumerate() {
        for (name, c) in terms {
            let hash = term_hash(name.as_bytes());
            *counts[kind].entry(hash).or_insert(0) += *c;
            edge_names[kind].insert(hash, name.as_str());
        }
    }
    if aligned {
        for (a, b) in minutes.iter_mut().zip(&edge.minutes) {
            *a += *b;
        }
    } else {
        let q = query::build(fields, &range_filter(group, start, end));
        minutes = searcher.search(q.as_ref(), &MinuteCollector { start, count: layout.minute_count })?;
    }

    let hourly_uv: Vec<usize> = hourly_sources
        .iter()
        .enumerate()
        .map(|(i, sources)| match (sources.as_slice(), edge.hourly_ips.get(i).filter(|s| !s.is_empty())) {
            ([], None) => 0,
            ([one], None) => one.len(),
            ([], Some(set)) => set.len(),
            (many, extra) => {
                let mut set = IdSet::default();
                for s in many {
                    set.extend(s.iter().copied());
                }
                if let Some(extra) = extra {
                    set.extend(extra.iter().copied());
                }
                set.len()
            }
        })
        .collect();

    let limits = [TOP_GROUP, TOP_GROUP, TOP_GROUP, TOP_URLS];
    let top: [Vec<(String, u64)>; 4] = std::array::from_fn(|kind| {
        top_of(&counts[kind], limits[kind], |hash| {
            edge_names[kind]
                .get(&hash)
                .map(|n| (*n).to_owned())
                .or_else(|| rollups.iter().find_map(|r| r.name(kind, hash).map(str::to_owned)))
        })
    });

    let figures = Figures {
        window_pv,
        window_bytes,
        window_uv: window_ips.len(),
        peak_minute: minutes.iter().copied().max().unwrap_or(0),
        hourly_pv,
        hourly_uv,
        daily_pv,
        daily_uv: daily_ips.iter().map(IdSet::len).collect(),
        top,
    };
    Ok(dashboard_response(&layout, &figures))
}

/// The `n` busiest terms of hash counts, by count and then by text, like
/// [`top_terms`]. Names are looked up only for the terms that can make it.
fn top_of(counts: &IdMap<u64>, n: usize, name: impl Fn(u64) -> Option<String>) -> Vec<(String, u64)> {
    let mut all: Vec<(u64, u64)> = counts.iter().map(|(h, c)| (*h, *c)).collect();
    all.sort_unstable_by_key(|e| std::cmp::Reverse(e.1));
    let cut = if all.len() > n && n > 0 { all[n - 1].1 } else { 0 };
    let mut named: Vec<(String, u64)> =
        all.into_iter().take_while(|(_, c)| *c >= cut).filter_map(|(h, c)| name(h).map(|t| (t, c))).collect();
    named.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    named.truncate(n);
    named
}

// ------------------------------------------------------------------- geo

/// A count with its share of the listed counts.
#[derive(Debug, PartialEq)]
pub struct Share {
    pub key: String,
    pub value: u64,
    pub percent: f64,
}

/// The top `size` terms with shares of their own total, sorted by count.
fn shares(counts: &HashMap<String, u64>, size: usize) -> Vec<Share> {
    let top = top_terms(counts, size);
    let total: u64 = top.iter().map(|(_, c)| c).sum();
    top.into_iter()
        .map(|(key, value)| Share {
            percent: if total > 0 { value as f64 / total as f64 * 100.0 } else { 0.0 },
            key,
            value,
        })
        .collect()
}

fn count_terms(
    searcher: &Searcher,
    fields: &Fields,
    filter: &Filter,
    field: &'static str,
) -> Result<HashMap<String, u64>, AnalyticsError> {
    let q = query::build(fields, filter);
    Ok(searcher.search(q.as_ref(), &TermCounts { field })?)
}

/// Requests per country.
pub fn countries(
    searcher: &Searcher,
    fields: &Fields,
    filter: &Filter,
    size: usize,
) -> Result<Vec<Share>, AnalyticsError> {
    Ok(shares_of_counts(&count_terms(searcher, fields, filter, "region_code")?, size))
}

/// Requests per province of a country.
pub fn provinces(
    searcher: &Searcher,
    fields: &Fields,
    filter: &Filter,
    country: &str,
    size: usize,
) -> Result<Vec<Share>, AnalyticsError> {
    let filter = Filter { countries: vec![country.to_owned()], ..filter.clone() };
    Ok(shares_of_counts(&count_terms(searcher, fields, &filter, "province")?, size))
}

/// Requests per city of a province.
pub fn cities(
    searcher: &Searcher,
    fields: &Fields,
    filter: &Filter,
    country: &str,
    province: &str,
    size: usize,
) -> Result<Vec<Share>, AnalyticsError> {
    let filter = Filter { countries: vec![country.to_owned()], provinces: vec![province.to_owned()], ..filter.clone() };
    Ok(shares_of_counts(&count_terms(searcher, fields, &filter, "city")?, size))
}

fn shares_of_counts(counts: &HashMap<String, u64>, size: usize) -> Vec<Share> {
    shares(counts, size)
}

/// The ten busiest city labels of a province, which carry the custom fields of
/// the geo database. Their shares are of all labels, not of the top ten.
pub fn city_labels(
    searcher: &Searcher,
    fields: &Fields,
    filter: &Filter,
    country: &str,
    province: &str,
) -> Result<Vec<Share>, AnalyticsError> {
    let filter = Filter { countries: vec![country.to_owned()], provinces: vec![province.to_owned()], ..filter.clone() };
    let q = query::build(fields, &filter);
    let counts = searcher.search(q.as_ref(), &CityLabels)?;
    let total: u64 = counts.values().sum();
    Ok(top_terms(&counts, 10)
        .into_iter()
        .map(|(key, value)| Share {
            percent: if total > 0 { value as f64 / total as f64 * 100.0 } else { 0.0 },
            key,
            value,
        })
        .collect())
}

// --------------------------------------------------------- entry statistics

#[derive(Debug, Serialize, PartialEq)]
pub struct KeyValue {
    pub key: String,
    pub value: u64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct BytesStatistics {
    pub total: u64,
    pub average: f64,
    pub min: u64,
    pub max: u64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ResponseTimeStatistics {
    pub average: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct EntriesStats {
    pub total_entries: u64,
    pub status_code_distribution: HashMap<String, u64>,
    pub method_distribution: HashMap<String, u64>,
    pub top_paths: Vec<KeyValue>,
    pub top_ips: Vec<KeyValue>,
    pub top_user_agents: Vec<KeyValue>,
    pub bytes_stats: BytesStatistics,
    pub response_time_stats: ResponseTimeStatistics,
}

fn key_values(counts: &HashMap<String, u64>, size: usize) -> Vec<KeyValue> {
    top_terms(counts, size).into_iter().map(|(key, value)| KeyValue { key, value }).collect()
}

/// Distributions, top lists and totals of the entries a filter matches.
pub fn entries_stats(searcher: &Searcher, fields: &Fields, filter: &Filter) -> Result<EntriesStats, AnalyticsError> {
    let q = query::build(fields, filter);
    let stats = searcher.search(q.as_ref(), &StatsCollector)?;
    let statuses = searcher.search(q.as_ref(), &NumCounts { field: "status" })?;
    let methods = count_terms(searcher, fields, filter, "method")?;
    let paths = count_terms(searcher, fields, filter, "path")?;
    let ips = count_terms(searcher, fields, filter, "ip")?;
    let agents = count_terms(searcher, fields, filter, "user_agent")?;

    let docs = stats.docs.max(1) as f64;
    Ok(EntriesStats {
        total_entries: stats.docs,
        status_code_distribution: top_terms(&statuses.into_iter().map(|(k, v)| (k.to_string(), v)).collect(), 10)
            .into_iter()
            .collect(),
        method_distribution: top_terms(&methods, 10).into_iter().collect(),
        top_paths: key_values(&paths, 10),
        top_ips: key_values(&ips, 10),
        top_user_agents: key_values(&agents, 10),
        bytes_stats: BytesStatistics {
            total: stats.total_bytes,
            average: stats.total_bytes as f64 / docs,
            min: stats.min_bytes.unwrap_or(0),
            max: stats.max_bytes,
        },
        response_time_stats: ResponseTimeStatistics {
            average: stats.total_request_time / docs,
            min: stats.min_request_time.unwrap_or(0.0),
            max: stats.max_request_time,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_follow_the_rules_of_the_go_plugin() {
        assert!(validate_range(0, 0).is_ok());
        assert!(validate_range(10, 0).is_ok());
        assert!(validate_range(10, 20).is_ok());
        assert!(matches!(validate_range(-1, 5), Err(AnalyticsError::NegativeTime)));
        assert!(matches!(validate_range(20, 10), Err(AnalyticsError::InvalidRange)));
        assert!(matches!(validate_range(10, 10), Err(AnalyticsError::InvalidRange)));
    }

    #[test]
    fn range_filter_treats_zero_as_open() {
        let f = range_filter("", 0, 0);
        assert!(f.groups.is_empty() && f.start.is_none() && f.end.is_none());
        let f = range_filter("/a.log", 5, 9);
        assert_eq!((f.start, f.end, f.groups), (Some(5), Some(9), vec!["/a.log".to_owned()]));
    }

    #[test]
    fn shares_sum_to_one_hundred_and_sort_by_count() {
        let counts: HashMap<String, u64> =
            [("US", 6u64), ("CN", 3), ("JP", 1)].into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        let s = shares(&counts, 300);
        assert_eq!(s.iter().map(|x| x.key.as_str()).collect::<Vec<_>>(), ["US", "CN", "JP"]);
        assert!((s.iter().map(|x| x.percent).sum::<f64>() - 100.0).abs() < 1e-9);
        assert_eq!(shares(&counts, 2).len(), 2);
    }

    #[test]
    fn an_empty_scan_gives_empty_lists_and_zero_figures() {
        let layout = Layout::new(1_000_000, 1_086_400);
        let scan = Figures::from(Dashboard::default());
        let r = dashboard_response(&layout, &scan);
        assert!(r.hourly_stats.is_empty() && r.daily_stats.is_empty() && r.top_urls.is_empty());
        assert_eq!(r.summary, DashboardSummary::default());
    }
}
