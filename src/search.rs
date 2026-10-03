//! Searching: the page of hits, the summary of the whole match set and the
//! entries as the pages show them.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use tantivy::collector::{Collector, TopDocs};
use tantivy::columnar::{Column, StrColumn};
use tantivy::query::Query;
use tantivy::schema::Value as _;
use tantivy::{DocAddress, Order, Searcher, TantivyDocument};

use crate::collectors::{Summary, SummaryCollector};
use crate::parse::{self, TimeCache};
use crate::query::{self, Filter};
use crate::schema::Fields;
use crate::useragent::{self, UaParser};

/// Most hits one request returns.
pub const MAX_LIMIT: usize = 10_000;
/// Hits of a request that sets no limit.
pub const DEFAULT_LIMIT: usize = 50;

/// What to search for and which page to return.
#[derive(Debug, Clone, Default)]
pub struct SearchParams {
    pub filter: Filter,
    pub limit: usize,
    pub offset: usize,
    /// A field of the hits, or `_score`. Empty means the timestamp.
    pub sort_by: String,
    pub descending: bool,
}

/// The page of hits and the figures of the whole match set.
pub struct SearchOutput {
    pub hits: Vec<DocAddress>,
    pub summary: Summary,
}

/// Runs the hits and the summary in one pass over the match set. Scores are
/// computed only for a request that sorts by score.
pub fn search(searcher: &Searcher, fields: &Fields, params: &SearchParams) -> tantivy::Result<SearchOutput> {
    let q = query::build(fields, &params.filter);
    let limit = if params.limit == 0 { DEFAULT_LIMIT } else { params.limit.min(MAX_LIMIT) };
    let order = if params.descending { Order::Desc } else { Order::Asc };
    let top = || TopDocs::with_limit(limit).and_offset(params.offset);

    fn run<C: Collector>(
        searcher: &Searcher,
        q: &dyn Query,
        collector: C,
        addresses: impl FnOnce(C::Fruit) -> Vec<DocAddress>,
    ) -> tantivy::Result<SearchOutput> {
        let (fruit, summary) = searcher.search(q, &(collector, SummaryCollector))?;
        Ok(SearchOutput { hits: addresses(fruit), summary })
    }

    match params.sort_by.as_str() {
        "_score" => run(searcher, q.as_ref(), top().order_by_score(), |h| h.into_iter().map(|(_, a)| a).collect()),
        "bytes_sent" => run(searcher, q.as_ref(), top().order_by_fast_field::<u64>("bytes_sent", order), |h| {
            h.into_iter().map(|(_, a)| a).collect()
        }),
        "status" => run(searcher, q.as_ref(), top().order_by_fast_field::<u64>("status", order), |h| {
            h.into_iter().map(|(_, a)| a).collect()
        }),
        "request_time" => run(searcher, q.as_ref(), top().order_by_fast_field::<f64>("request_time", order), |h| {
            h.into_iter().map(|(_, a)| a).collect()
        }),
        name @ ("ip" | "method" | "browser" | "os" | "device_type" | "region_code" | "province" | "city" | "level") => {
            run(searcher, q.as_ref(), top().order_by_string_fast_field(name, order), |h| {
                h.into_iter().map(|(_, a)| a).collect()
            })
        }
        _ => run(searcher, q.as_ref(), top().order_by_fast_field::<i64>("ts", order), |h| {
            h.into_iter().map(|(_, a)| a).collect()
        }),
    }
}

/// Columns of one segment the entries read.
struct SegmentColumns {
    ts: Option<Column<i64>>,
    ip: Option<StrColumn>,
    level: Option<StrColumn>,
    geo: [Option<StrColumn>; 7],
    /// The ISO 3166-2 code of the region and the GeoNames id of the city, by
    /// which the page names them in its language.
    sub1: Option<StrColumn>,
    city_id: Option<Column<u64>>,
}

const GEO_FIELDS: [&str; 7] = ["region_code", "province", "city", "c1", "c2", "c3", "c4"];

/// Builds the entries of hits. The line is parsed again for the request, the
/// user agent and the times, the location comes from the index since it depends
/// on the database of the day the line was indexed.
pub struct EntryLoader<'a> {
    searcher: &'a Searcher,
    fields: &'a Fields,
    ua: Arc<UaParser>,
    columns: HashMap<u32, SegmentColumns>,
    times: TimeCache,
    error_times: TimeCache,
}

fn text_at(col: &Option<StrColumn>, doc: u32, buf: &mut String) -> String {
    let Some(col) = col else { return String::new() };
    let Some(ord) = col.ords().first(doc) else { return String::new() };
    buf.clear();
    if col.ord_to_str(ord, buf).unwrap_or(false) {
        buf.clone()
    } else {
        String::new()
    }
}

impl<'a> EntryLoader<'a> {
    pub fn new(searcher: &'a Searcher, fields: &'a Fields) -> Self {
        Self {
            searcher,
            fields,
            ua: useragent::parser().clone(),
            columns: HashMap::new(),
            times: TimeCache::default(),
            error_times: TimeCache::default(),
        }
    }

    fn columns(&mut self, segment: u32) -> tantivy::Result<&SegmentColumns> {
        if !self.columns.contains_key(&segment) {
            let reader = self.searcher.segment_reader(segment);
            let ff = reader.fast_fields();
            let mut geo: [Option<StrColumn>; 7] = Default::default();
            for (slot, name) in geo.iter_mut().zip(GEO_FIELDS) {
                *slot = ff.str(name)?;
            }
            self.columns.insert(
                segment,
                SegmentColumns {
                    ts: ff.column_opt::<i64>("ts")?,
                    ip: ff.str("ip")?,
                    level: ff.str("level")?,
                    geo,
                    sub1: ff.str("sub1")?,
                    city_id: ff.column_opt::<u64>("city_id")?,
                },
            );
        }
        Ok(&self.columns[&segment])
    }

    /// The entry of one hit.
    pub fn load(&mut self, address: DocAddress) -> tantivy::Result<Map<String, Value>> {
        let doc: TantivyDocument = self.searcher.doc(address)?;
        let raw = doc.get_first(self.fields.raw).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        let cols = self.columns(address.segment_ord)?;
        let mut buf = String::new();
        let ts = cols.ts.as_ref().and_then(|c| c.first(address.doc_id)).unwrap_or(0);
        let ip = text_at(&cols.ip, address.doc_id, &mut buf);
        let level = text_at(&cols.level, address.doc_id, &mut buf);
        if !level.is_empty() {
            return Ok(error_entry(ts, &level, &raw, &mut self.error_times));
        }
        let mut geo: Vec<String> = Vec::with_capacity(7);
        for col in &cols.geo {
            geo.push(text_at(col, address.doc_id, &mut buf));
        }
        let sub1 = text_at(&cols.sub1, address.doc_id, &mut buf);
        let city_id = cols.city_id.as_ref().and_then(|c| c.first(address.doc_id)).unwrap_or_default();

        let parsed = parse::parse_line(&raw, &mut self.times);
        let (method, path, protocol, status, bytes, referer, user_agent, request_time, upstream_time) = match &parsed {
            Some(e) => (
                e.method,
                e.path,
                e.protocol,
                e.status,
                e.bytes_sent,
                e.referer,
                e.user_agent,
                e.request_time,
                e.upstream_time,
            ),
            None => ("", "", "", 0, 0, "", "", None, None),
        };
        let ua = if user_agent.is_empty() { Default::default() } else { self.ua.detail(user_agent) };

        let mut m = Map::new();
        m.insert("timestamp".into(), json!(ts));
        m.insert("ip".into(), json!(if ip.is_empty() { parsed.as_ref().map_or("", |e| e.ip) } else { ip.as_str() }));
        m.insert("method".into(), json!(method));
        m.insert("path".into(), json!(path));
        m.insert("protocol".into(), json!(protocol));
        m.insert("status".into(), json!(status));
        m.insert("bytes_sent".into(), json!(bytes));
        m.insert("referer".into(), json!(referer));
        m.insert("user_agent".into(), json!(user_agent));
        m.insert("browser".into(), json!(ua.browser));
        m.insert("browser_version".into(), json!(ua.browser_version));
        m.insert("os".into(), json!(ua.os));
        m.insert("os_version".into(), json!(ua.os_version));
        m.insert("device_type".into(), json!(ua.device));
        for (name, value) in GEO_FIELDS.iter().zip(&geo) {
            // The custom fields are left out when empty
            if name.starts_with('c') && name.len() == 2 && value.is_empty() {
                continue;
            }
            m.insert((*name).into(), json!(value));
        }
        m.insert("sub1".into(), json!(sub1));
        m.insert("city_id".into(), json!(city_id));
        if let Some(t) = request_time.filter(|t| *t > 0.0) {
            m.insert("request_time".into(), json!(t));
        }
        if let Some(t) = upstream_time {
            m.insert("upstream_time".into(), json!(t));
        }
        m.insert("raw".into(), json!(raw));
        Ok(m)
    }
}

/// The entry of an error log hit, read from its line. `referer` and `ip` use
/// the names of the access entries so the pages share their cells.
fn error_entry(ts: i64, level: &str, raw: &str, times: &mut TimeCache) -> Map<String, Value> {
    let parsed = parse::parse_error_line(raw, times).unwrap_or_default();
    let mut m = Map::new();
    m.insert("timestamp".into(), json!(ts));
    m.insert("level".into(), json!(level));
    m.insert("pid".into(), json!(parsed.pid));
    m.insert("connection".into(), json!(parsed.connection));
    m.insert("message".into(), json!(parsed.message));
    m.insert("ip".into(), json!(parsed.client));
    m.insert("server".into(), json!(parsed.server));
    m.insert("request".into(), json!(parsed.request));
    m.insert("method".into(), json!(parsed.method));
    m.insert("path".into(), json!(parsed.path));
    m.insert("upstream".into(), json!(parsed.upstream));
    m.insert("host".into(), json!(parsed.host));
    m.insert("referer".into(), json!(parsed.referrer));
    m.insert("raw".into(), json!(raw));
    m
}

fn text_of(entry: &Map<String, Value>, key: &str) -> String {
    entry.get(key).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_owned()
}

/// The location of an entry as one label: country, province and city, then the
/// custom fields. A Chinese page shows the country code CN as its name.
pub fn location_label(entry: &Map<String, Value>, chinese: bool) -> String {
    let code = text_of(entry, "region_code");
    let region = if !chinese {
        code
    } else if code == "CN" {
        "中国".to_owned()
    } else {
        code
    };
    let parts: Vec<String> = [region, text_of(entry, "province"), text_of(entry, "city")]
        .into_iter()
        .chain(["c1", "c2", "c3", "c4"].iter().map(|k| text_of(entry, k)))
        .filter(|p| !p.is_empty())
        .collect();
    // Chinese names read as one phrase with spaces, others as a list
    parts.join(if chinese { " " } else { ", " })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), json!(v))).collect()
    }

    #[test]
    fn location_label_joins_the_parts() {
        let e = entry(&[("region_code", "CN"), ("province", "广东"), ("city", "深圳"), ("c1", "电信")]);
        assert_eq!(location_label(&e, false), "CN, 广东, 深圳, 电信");
        assert_eq!(location_label(&e, true), "中国 广东 深圳 电信");
        let e = entry(&[("region_code", "US")]);
        assert_eq!(location_label(&e, true), "US");
        let e = entry(&[("c1", "corp"), ("c2", "lan")]);
        assert_eq!(location_label(&e, false), "corp, lan");
        assert_eq!(location_label(&entry(&[]), false), "");
    }
}
