//! Collectors over the fast field columns: the summary of a match set, term
//! counts, numeric statistics, the time range and the dashboard scan.
//!
//! Distinct values are counted exactly. A segment marks the ordinals it saw in a
//! bitset and one pass over its dictionary turns them into 64 bit term hashes,
//! which merge across segments, so the count is exact up to hash collisions.

use std::collections::{HashMap, HashSet};

use tantivy::collector::{Collector, SegmentCollector};
use tantivy::columnar::{Column, StrColumn};
use tantivy::{DocId, Score, SegmentReader};

use crate::localtime;

/// Plain bitset over term ordinals.
#[derive(Default, Clone)]
pub struct Bits(Vec<u64>);

impl Bits {
    fn with_len(n: usize) -> Self {
        Bits(vec![0; n.div_ceil(64)])
    }

    #[inline]
    fn set(&mut self, i: u64) {
        self.0[(i >> 6) as usize] |= 1 << (i & 63);
    }

    fn ones(&self) -> impl Iterator<Item = u64> + '_ {
        self.0.iter().enumerate().flat_map(|(w, &bits)| {
            let mut b = bits;
            std::iter::from_fn(move || {
                if b == 0 {
                    return None;
                }
                let t = b.trailing_zeros() as u64;
                b &= b - 1;
                Some((w as u64) * 64 + t)
            })
        })
    }

    fn or_with(&mut self, other: &Bits) {
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            *a |= *b;
        }
    }
}

fn hash_bytes(b: &[u8]) -> u64 {
    crate::rollup::term_hash(b)
}

/// Maps the ordinals of one segment to term hashes in one dictionary pass.
fn ords_to_hashes(col: &StrColumn, ords: &Bits) -> HashMap<u64, u64> {
    let list: Vec<u64> = ords.ones().collect();
    let mut out = HashMap::with_capacity(list.len());
    let mut it = list.iter();
    let _ = col.dictionary().sorted_ords_to_term_cb(list.iter().copied(), |term| {
        if let Some(ord) = it.next() {
            out.insert(*ord, hash_bytes(term));
        }
        Ok(())
    });
    out
}

/// Terms and counts for the ordinals that have a count.
fn counted_terms(col: &StrColumn, counts: &[u32]) -> HashMap<String, u64> {
    let list: Vec<u64> = counts.iter().enumerate().filter(|(_, c)| **c > 0).map(|(i, _)| i as u64).collect();
    let mut out = HashMap::with_capacity(list.len());
    let mut it = list.iter();
    let _ = col.dictionary().sorted_ords_to_term_cb(list.iter().copied(), |term| {
        if let Some(ord) = it.next() {
            *out.entry(String::from_utf8_lossy(term).into_owned()).or_insert(0) += u64::from(counts[*ord as usize]);
        }
        Ok(())
    });
    out
}

fn str_column(reader: &SegmentReader, name: &str) -> tantivy::Result<Option<StrColumn>> {
    reader.fast_fields().str(name)
}

fn column<T>(reader: &SegmentReader, name: &str) -> tantivy::Result<Option<Column<T>>>
where
    T: tantivy::columnar::HasAssociatedColumnType + PartialOrd + Copy + Send + Sync + 'static,
    tantivy::columnar::DynamicColumn: Into<Option<Column<T>>>,
{
    reader.fast_fields().column_opt::<T>(name)
}

/// Top `n` of counted terms, highest count first and by term for equal counts.
pub fn top_terms(counts: &HashMap<String, u64>, n: usize) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = counts.iter().map(|(k, c)| (k.clone(), *c)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

// ---------------------------------------------------------------- summary

/// Hits, bytes and distinct visitors and pages of a match set.
#[derive(Default)]
pub struct Summary {
    pub docs: u64,
    pub bytes: u64,
    pub ips: HashSet<u64>,
    pub pages: HashSet<u64>,
}

pub struct SummaryCollector;

pub struct SummarySegment {
    ip: Option<StrColumn>,
    page: Option<StrColumn>,
    bytes: Option<Column<u64>>,
    seen_ip: Bits,
    seen_page: Bits,
    docs: u64,
    total: u64,
}

impl Collector for SummaryCollector {
    type Fruit = Summary;
    type Child = SummarySegment;

    fn for_segment(&self, _id: u32, reader: &SegmentReader) -> tantivy::Result<SummarySegment> {
        let ip = str_column(reader, "ip")?;
        let page = str_column(reader, "path")?;
        Ok(SummarySegment {
            seen_ip: Bits::with_len(ip.as_ref().map_or(0, |c| c.num_terms())),
            seen_page: Bits::with_len(page.as_ref().map_or(0, |c| c.num_terms())),
            ip,
            page,
            bytes: column::<u64>(reader, "bytes_sent")?,
            docs: 0,
            total: 0,
        })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(&self, fruits: Vec<Summary>) -> tantivy::Result<Summary> {
        let mut out = Summary::default();
        for f in fruits {
            out.docs += f.docs;
            out.bytes += f.bytes;
            out.ips.extend(f.ips);
            out.pages.extend(f.pages);
        }
        Ok(out)
    }
}

impl SegmentCollector for SummarySegment {
    type Fruit = Summary;

    fn collect(&mut self, doc: DocId, _score: Score) {
        self.docs += 1;
        if let Some(b) = &self.bytes {
            self.total += b.first(doc).unwrap_or(0);
        }
        if let Some(o) = self.ip.as_ref().and_then(|c| c.ords().first(doc)) {
            self.seen_ip.set(o);
        }
        if let Some(o) = self.page.as_ref().and_then(|c| c.ords().first(doc)) {
            self.seen_page.set(o);
        }
    }

    fn harvest(self) -> Summary {
        let set_of = |col: &Option<StrColumn>, seen: &Bits| -> HashSet<u64> {
            match col {
                Some(col) => ords_to_hashes(col, seen).into_values().collect(),
                None => HashSet::new(),
            }
        };
        Summary {
            docs: self.docs,
            bytes: self.total,
            ips: set_of(&self.ip, &self.seen_ip),
            pages: set_of(&self.page, &self.seen_page),
        }
    }
}

// ------------------------------------------------------------ term counts

/// Counts of the values of a string fast field over a match set.
pub struct TermCounts {
    pub field: &'static str,
}

pub struct TermCountsSegment {
    col: Option<StrColumn>,
    counts: Vec<u32>,
}

impl Collector for TermCounts {
    type Fruit = HashMap<String, u64>;
    type Child = TermCountsSegment;

    fn for_segment(&self, _id: u32, reader: &SegmentReader) -> tantivy::Result<TermCountsSegment> {
        let col = str_column(reader, self.field)?;
        let counts = vec![0u32; col.as_ref().map_or(0, |c| c.num_terms())];
        Ok(TermCountsSegment { col, counts })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(&self, fruits: Vec<Self::Fruit>) -> tantivy::Result<Self::Fruit> {
        let mut out: HashMap<String, u64> = HashMap::new();
        for f in fruits {
            for (k, c) in f {
                *out.entry(k).or_insert(0) += c;
            }
        }
        Ok(out)
    }
}

impl SegmentCollector for TermCountsSegment {
    type Fruit = HashMap<String, u64>;

    fn collect(&mut self, doc: DocId, _score: Score) {
        if let Some(o) = self.col.as_ref().and_then(|c| c.ords().first(doc)) {
            self.counts[o as usize] += 1;
        }
    }

    fn harvest(self) -> Self::Fruit {
        match &self.col {
            Some(col) => counted_terms(col, &self.counts),
            None => HashMap::new(),
        }
    }
}

/// Counts of the values of a u64 fast field, such as the status code.
pub struct NumCounts {
    pub field: &'static str,
}

pub struct NumCountsSegment {
    col: Option<Column<u64>>,
    counts: HashMap<u64, u64>,
}

impl Collector for NumCounts {
    type Fruit = HashMap<u64, u64>;
    type Child = NumCountsSegment;

    fn for_segment(&self, _id: u32, reader: &SegmentReader) -> tantivy::Result<NumCountsSegment> {
        Ok(NumCountsSegment { col: column::<u64>(reader, self.field)?, counts: HashMap::new() })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(&self, fruits: Vec<Self::Fruit>) -> tantivy::Result<Self::Fruit> {
        let mut out: HashMap<u64, u64> = HashMap::new();
        for f in fruits {
            for (k, c) in f {
                *out.entry(k).or_insert(0) += c;
            }
        }
        Ok(out)
    }
}

impl SegmentCollector for NumCountsSegment {
    type Fruit = HashMap<u64, u64>;

    fn collect(&mut self, doc: DocId, _score: Score) {
        if let Some(v) = self.col.as_ref().and_then(|c| c.first(doc)) {
            *self.counts.entry(v).or_insert(0) += 1;
        }
    }

    fn harvest(self) -> Self::Fruit {
        self.counts
    }
}

// ------------------------------------------------------------- label counts

/// Counts of the city labels of a match set. A label is the city, followed by
/// the custom fields of the geo database that are set, joined by ` · `.
pub struct CityLabels;

pub struct CityLabelsSegment {
    cols: [Option<StrColumn>; 5],
    counts: HashMap<[u64; 5], u64>,
}

const MISSING: u64 = u64::MAX;

impl Collector for CityLabels {
    type Fruit = HashMap<String, u64>;
    type Child = CityLabelsSegment;

    fn for_segment(&self, _id: u32, reader: &SegmentReader) -> tantivy::Result<CityLabelsSegment> {
        let names = ["city", "c1", "c2", "c3", "c4"];
        let mut cols: [Option<StrColumn>; 5] = Default::default();
        for (slot, name) in cols.iter_mut().zip(names) {
            *slot = str_column(reader, name)?;
        }
        Ok(CityLabelsSegment { cols, counts: HashMap::new() })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(&self, fruits: Vec<Self::Fruit>) -> tantivy::Result<Self::Fruit> {
        let mut out: HashMap<String, u64> = HashMap::new();
        for f in fruits {
            for (k, c) in f {
                *out.entry(k).or_insert(0) += c;
            }
        }
        Ok(out)
    }
}

impl SegmentCollector for CityLabelsSegment {
    type Fruit = HashMap<String, u64>;

    fn collect(&mut self, doc: DocId, _score: Score) {
        let mut key = [MISSING; 5];
        for (slot, col) in key.iter_mut().zip(&self.cols) {
            if let Some(o) = col.as_ref().and_then(|c| c.ords().first(doc)) {
                *slot = o;
            }
        }
        if key[0] != MISSING {
            *self.counts.entry(key).or_insert(0) += 1;
        }
    }

    fn harvest(self) -> Self::Fruit {
        let mut out: HashMap<String, u64> = HashMap::new();
        let mut buf = String::new();
        for (key, count) in self.counts {
            let mut parts: Vec<String> = Vec::new();
            for (ord, col) in key.iter().zip(&self.cols) {
                let (Some(col), true) = (col, *ord != MISSING) else { continue };
                buf.clear();
                if col.ord_to_str(*ord, &mut buf).unwrap_or(false) {
                    let t = buf.trim();
                    if !t.is_empty() {
                        parts.push(t.to_owned());
                    }
                }
            }
            if parts.is_empty() || key[0] == MISSING {
                continue;
            }
            // The first part is the city, empty cities are not counted
            *out.entry(parts.join(" · ")).or_insert(0) += count;
        }
        out
    }
}

// ------------------------------------------------------------------ stats

/// Sums and extremes of the bytes and request times of a match set.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stats {
    pub docs: u64,
    pub total_bytes: u64,
    pub min_bytes: Option<u64>,
    pub max_bytes: u64,
    pub total_request_time: f64,
    pub min_request_time: Option<f64>,
    pub max_request_time: f64,
}

pub struct StatsCollector;

pub struct StatsSegment {
    bytes: Option<Column<u64>>,
    time: Option<Column<f64>>,
    out: Stats,
}

impl Collector for StatsCollector {
    type Fruit = Stats;
    type Child = StatsSegment;

    fn for_segment(&self, _id: u32, reader: &SegmentReader) -> tantivy::Result<StatsSegment> {
        Ok(StatsSegment {
            bytes: column::<u64>(reader, "bytes_sent")?,
            time: column::<f64>(reader, "request_time")?,
            out: Stats::default(),
        })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(&self, fruits: Vec<Stats>) -> tantivy::Result<Stats> {
        let mut out = Stats::default();
        for f in fruits {
            out.docs += f.docs;
            out.total_bytes += f.total_bytes;
            out.min_bytes = match (out.min_bytes, f.min_bytes) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            out.max_bytes = out.max_bytes.max(f.max_bytes);
            out.total_request_time += f.total_request_time;
            out.min_request_time = match (out.min_request_time, f.min_request_time) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            out.max_request_time = out.max_request_time.max(f.max_request_time);
        }
        Ok(out)
    }
}

impl SegmentCollector for StatsSegment {
    type Fruit = Stats;

    fn collect(&mut self, doc: DocId, _score: Score) {
        self.out.docs += 1;
        if let Some(b) = self.bytes.as_ref().and_then(|c| c.first(doc)) {
            self.out.total_bytes += b;
            self.out.min_bytes = Some(self.out.min_bytes.map_or(b, |m| m.min(b)));
            self.out.max_bytes = self.out.max_bytes.max(b);
        }
        if let Some(t) = self.time.as_ref().and_then(|c| c.first(doc)) {
            self.out.total_request_time += t;
            self.out.min_request_time = Some(self.out.min_request_time.map_or(t, |m| m.min(t)));
            self.out.max_request_time = self.out.max_request_time.max(t);
        }
    }

    fn harvest(self) -> Stats {
        self.out
    }
}

// ------------------------------------------------------------- time range

/// Smallest and largest timestamp of a match set.
pub struct TimeRangeCollector;

pub struct TimeRangeSegment {
    ts: Option<Column<i64>>,
    range: Option<(i64, i64)>,
}

impl Collector for TimeRangeCollector {
    type Fruit = Option<(i64, i64)>;
    type Child = TimeRangeSegment;

    fn for_segment(&self, _id: u32, reader: &SegmentReader) -> tantivy::Result<TimeRangeSegment> {
        Ok(TimeRangeSegment { ts: column::<i64>(reader, "ts")?, range: None })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(&self, fruits: Vec<Self::Fruit>) -> tantivy::Result<Self::Fruit> {
        Ok(fruits.into_iter().flatten().reduce(|a, b| (a.0.min(b.0), a.1.max(b.1))))
    }
}

impl SegmentCollector for TimeRangeSegment {
    type Fruit = Option<(i64, i64)>;

    fn collect(&mut self, doc: DocId, _score: Score) {
        if let Some(t) = self.ts.as_ref().and_then(|c| c.first(doc)) {
            self.range = Some(self.range.map_or((t, t), |(lo, hi)| (lo.min(t), hi.max(t))));
        }
    }

    fn harvest(self) -> Self::Fruit {
        self.range
    }
}

// -------------------------------------------------------------- dashboard

/// Bucket layout of one dashboard scan: daily buckets in server local time and
/// an hourly UTC grid with a buffer of 12 hours on each side, so the page can
/// show its own time zone.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The window `[start, end)` of the figures.
    pub start: i64,
    pub end: i64,
    pub hour_start: i64,
    pub hour_count: usize,
    /// Local calendar days of the window as `[first instant, next day's first instant)`.
    pub days: Vec<(i64, i64)>,
    /// Local date label (`YYYY-MM-DD`) and first stepped timestamp of each day.
    pub day_labels: Vec<(String, i64)>,
    pub minute_count: usize,
}

impl Layout {
    pub fn new(start: i64, end: i64) -> Self {
        let hour_start = start - 12 * 3600;
        let hour_end = end + 12 * 3600;
        // One bucket per local date, found by stepping whole days from the start
        let mut days = Vec::new();
        let mut day_labels = Vec::new();
        let mut seen: Vec<i32> = Vec::new();
        let mut t = start;
        while t < end {
            let key = localtime::date_key(t);
            if !seen.contains(&key) {
                let lo = localtime::local_midnight(t);
                days.push((lo, localtime::local_midnight(lo + 36 * 3600)));
                day_labels.push((localtime::format_date(t), t));
                seen.push(key);
            }
            t = localtime::add_local_day(t);
        }
        let hour_count = if hour_end > hour_start { ((hour_end - hour_start + 3599) / 3600) as usize } else { 0 };
        Layout {
            start,
            end,
            hour_start,
            hour_count,
            days,
            day_labels,
            minute_count: ((end - start).max(0) as usize).div_ceil(60),
        }
    }

    /// Range the scan query covers, both ends inclusive.
    pub fn scan_range(&self) -> (i64, i64) {
        (self.hour_start, self.end + 12 * 3600)
    }
}

/// Figures of a dashboard scan.
#[derive(Default)]
pub struct Dashboard {
    pub window_pv: u64,
    pub window_bytes: u64,
    pub window_ips: HashSet<u64>,
    pub minutes: Vec<u32>,
    pub hourly_pv: Vec<u64>,
    pub hourly_ips: Vec<HashSet<u64>>,
    pub daily_pv: Vec<u64>,
    pub daily_ips: Vec<HashSet<u64>>,
    /// Counts of browser, os, device type and path.
    pub groups: [HashMap<String, u64>; 4],
}

impl Dashboard {
    fn empty(l: &Layout) -> Self {
        Dashboard {
            minutes: vec![0; l.minute_count],
            hourly_pv: vec![0; l.hour_count],
            hourly_ips: vec![HashSet::new(); l.hour_count],
            daily_pv: vec![0; l.days.len()],
            daily_ips: vec![HashSet::new(); l.days.len()],
            ..Default::default()
        }
    }

    pub fn peak_minute(&self) -> u32 {
        self.minutes.iter().copied().max().unwrap_or(0)
    }
}

pub struct DashboardCollector {
    pub layout: Layout,
}

const GROUP_FIELDS: [&str; 4] = ["browser", "os", "device_type", "path"];

pub struct DashboardSegment {
    layout: Layout,
    ts: Option<Column<i64>>,
    bytes: Option<Column<u64>>,
    ip: Option<StrColumn>,
    groups: [Option<StrColumn>; 4],
    group_counts: [Vec<u32>; 4],
    window_ips: Bits,
    hourly_ips: Vec<Option<Bits>>,
    daily_ips: Vec<Option<Bits>>,
    out: Dashboard,
    num_ips: usize,
    last_day: usize,
}

impl Collector for DashboardCollector {
    type Fruit = Dashboard;
    type Child = DashboardSegment;

    fn for_segment(&self, _id: u32, reader: &SegmentReader) -> tantivy::Result<DashboardSegment> {
        let ip = str_column(reader, "ip")?;
        let mut groups: [Option<StrColumn>; 4] = Default::default();
        for (slot, name) in groups.iter_mut().zip(GROUP_FIELDS) {
            *slot = str_column(reader, name)?;
        }
        let counts = |i: usize| vec![0u32; groups[i].as_ref().map_or(0, |c| c.num_terms())];
        let group_counts = [counts(0), counts(1), counts(2), counts(3)];
        let num_ips = ip.as_ref().map_or(0, |c| c.num_terms());
        Ok(DashboardSegment {
            layout: self.layout.clone(),
            ts: column::<i64>(reader, "ts")?,
            bytes: column::<u64>(reader, "bytes_sent")?,
            ip,
            groups,
            group_counts,
            window_ips: Bits::with_len(num_ips),
            hourly_ips: vec![None; self.layout.hour_count],
            daily_ips: vec![None; self.layout.days.len()],
            out: Dashboard::empty(&self.layout),
            num_ips,
            last_day: 0,
        })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(&self, fruits: Vec<Dashboard>) -> tantivy::Result<Dashboard> {
        let mut out = Dashboard::empty(&self.layout);
        for f in fruits {
            out.window_pv += f.window_pv;
            out.window_bytes += f.window_bytes;
            out.window_ips.extend(f.window_ips);
            for (a, b) in out.minutes.iter_mut().zip(&f.minutes) {
                *a += *b;
            }
            for (a, b) in out.hourly_pv.iter_mut().zip(&f.hourly_pv) {
                *a += *b;
            }
            for (a, b) in out.hourly_ips.iter_mut().zip(f.hourly_ips) {
                a.extend(b);
            }
            for (a, b) in out.daily_pv.iter_mut().zip(&f.daily_pv) {
                *a += *b;
            }
            for (a, b) in out.daily_ips.iter_mut().zip(f.daily_ips) {
                a.extend(b);
            }
            for (a, b) in out.groups.iter_mut().zip(f.groups) {
                for (k, c) in b {
                    *a.entry(k).or_insert(0) += c;
                }
            }
        }
        Ok(out)
    }
}

fn set_bit(slot: &mut Option<Bits>, n: usize, ord: u64) {
    slot.get_or_insert_with(|| Bits::with_len(n)).set(ord);
}

impl DashboardSegment {
    /// Day bucket of a timestamp. The last hit is cached, logs are mostly in time order.
    fn day_of(&mut self, ts: i64) -> Option<usize> {
        let days = &self.layout.days;
        if let Some(&(lo, hi)) = days.get(self.last_day) {
            if ts >= lo && ts < hi {
                return Some(self.last_day);
            }
        }
        let i = days.partition_point(|d| d.0 <= ts).checked_sub(1)?;
        if ts < days[i].1 {
            self.last_day = i;
            Some(i)
        } else {
            None
        }
    }
}

impl SegmentCollector for DashboardSegment {
    type Fruit = Dashboard;

    fn collect(&mut self, doc: DocId, _score: Score) {
        let Some(ts) = self.ts.as_ref().and_then(|c| c.first(doc)) else { return };
        let ord = self.ip.as_ref().and_then(|c| c.ords().first(doc));
        let (l_start, l_end, l_hour_start, l_hour_count) =
            (self.layout.start, self.layout.end, self.layout.hour_start, self.layout.hour_count);

        if ts >= l_start && ts < l_end {
            self.out.window_pv += 1;
            self.out.window_bytes += self.bytes.as_ref().and_then(|c| c.first(doc)).unwrap_or(0);
            self.out.minutes[((ts - l_start) / 60) as usize] += 1;
            if let Some(o) = ord {
                self.window_ips.set(o);
            }
            for i in 0..4 {
                if let Some(o) = self.groups[i].as_ref().and_then(|c| c.ords().first(doc)) {
                    self.group_counts[i][o as usize] += 1;
                }
            }
        }

        if let Some(day) = self.day_of(ts) {
            self.out.daily_pv[day] += 1;
            if let Some(o) = ord {
                set_bit(&mut self.daily_ips[day], self.num_ips, o);
            }
        }
        let hour = ts - ts.rem_euclid(3600) - l_hour_start;
        if hour >= 0 && (hour / 3600) < l_hour_count as i64 {
            let h = (hour / 3600) as usize;
            self.out.hourly_pv[h] += 1;
            if let Some(o) = ord {
                set_bit(&mut self.hourly_ips[h], self.num_ips, o);
            }
        }
    }

    fn harvest(mut self) -> Dashboard {
        if let Some(col) = &self.ip {
            // One dictionary pass for every ordinal seen in any bucket
            let mut all = self.window_ips.clone();
            for b in self.hourly_ips.iter().chain(self.daily_ips.iter()).flatten() {
                all.or_with(b);
            }
            let hashes = ords_to_hashes(col, &all);
            let to_set = |b: &Bits| -> HashSet<u64> { b.ones().filter_map(|o| hashes.get(&o).copied()).collect() };
            self.out.window_ips = to_set(&self.window_ips);
            for (i, b) in self.hourly_ips.iter().enumerate() {
                if let Some(b) = b {
                    self.out.hourly_ips[i] = to_set(b);
                }
            }
            for (i, b) in self.daily_ips.iter().enumerate() {
                if let Some(b) = b {
                    self.out.daily_ips[i] = to_set(b);
                }
            }
        }
        for i in 0..4 {
            if let Some(col) = &self.groups[i] {
                self.out.groups[i] = counted_terms(col, &self.group_counts[i]);
            }
        }
        self.out
    }
}

// ---------------------------------------------------------------- minutes

/// Views per minute of a window, counted from its start.
pub struct MinuteCollector {
    pub start: i64,
    pub count: usize,
}

pub struct MinuteSegment {
    ts: Option<Column<i64>>,
    start: i64,
    counts: Vec<u32>,
}

impl Collector for MinuteCollector {
    type Fruit = Vec<u32>;
    type Child = MinuteSegment;

    fn for_segment(&self, _id: u32, reader: &SegmentReader) -> tantivy::Result<MinuteSegment> {
        Ok(MinuteSegment { ts: column::<i64>(reader, "ts")?, start: self.start, counts: vec![0; self.count] })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(&self, fruits: Vec<Vec<u32>>) -> tantivy::Result<Vec<u32>> {
        let mut out = vec![0u32; self.count];
        for f in fruits {
            for (a, b) in out.iter_mut().zip(f) {
                *a += b;
            }
        }
        Ok(out)
    }
}

impl SegmentCollector for MinuteSegment {
    type Fruit = Vec<u32>;

    fn collect(&mut self, doc: DocId, _score: Score) {
        let Some(ts) = self.ts.as_ref().and_then(|c| c.first(doc)) else { return };
        if ts >= self.start {
            if let Some(slot) = self.counts.get_mut(((ts - self.start) / 60) as usize) {
                *slot += 1;
            }
        }
    }

    fn harvest(self) -> Vec<u32> {
        self.counts
    }
}
