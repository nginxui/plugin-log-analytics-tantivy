//! The figures of the dashboard, the maps and the entry statistics.

use std::collections::HashMap;

use serde::Serialize;
use tantivy::Searcher;

use crate::collectors::{
    top_terms, CityLabels, Dashboard, DashboardCollector, Layout, NumCounts, StatsCollector, TermCounts,
};
use crate::query::{self, Filter};
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

/// Assembles the dashboard response from a scan.
pub fn dashboard_response(layout: &Layout, scan: &Dashboard) -> DashboardResponse {
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
                    uv: scan.hourly_ips[i].len(),
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
                uv: scan.daily_ips[i].len(),
                pv: scan.daily_pv[i],
                timestamp: *stamp,
            })
            .collect();
        response.daily_stats.sort_by_key(|d| d.timestamp);
        response.browsers = top_terms(&scan.groups[0], TOP_GROUP)
            .into_iter()
            .map(|(browser, count)| BrowserStats { percent: percent(count, total_pv), browser, count })
            .collect();
        response.operating_systems = top_terms(&scan.groups[1], TOP_GROUP)
            .into_iter()
            .map(|(os, count)| OsStats { percent: percent(count, total_pv), os, count })
            .collect();
        response.devices = top_terms(&scan.groups[2], TOP_GROUP)
            .into_iter()
            .map(|(device, count)| DeviceStats { percent: percent(count, total_pv), device, count })
            .collect();
        response.top_urls = top_terms(&scan.groups[3], TOP_URLS)
            .into_iter()
            .map(|(url, visits)| UrlStats { percent: percent(visits, total_pv), url, visits })
            .collect();
    }

    let days = response.daily_stats.len();
    let total_uv = scan.window_ips.len();
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
        peak_qps: f64::from(scan.peak_minute()) / 60.0,
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
    Ok(dashboard_response(&layout, &scan))
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
        let scan = Dashboard::default();
        let r = dashboard_response(&layout, &scan);
        assert!(r.hourly_stats.is_empty() && r.daily_stats.is_empty() && r.top_urls.is_empty());
        assert_eq!(r.summary, DashboardSummary::default());
    }
}
