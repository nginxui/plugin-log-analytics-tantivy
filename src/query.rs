//! Turns the filters of a request into a tantivy query.
//!
//! The search box follows [`crate::qsyntax`]: its words are analyzed like the
//! raw line and every token has to match. The path, user agent and referer
//! filters are phrases over their analyzed field. The time range includes its
//! start and excludes its end.

use std::net::Ipv6Addr;
use std::ops::Bound;

use tantivy::query::{
    AllQuery, BooleanQuery, EmptyQuery, Occur, PhraseQuery, Query, RangeQuery, RegexQuery, TermQuery,
};
use tantivy::schema::{Field, IndexRecordOption, Term};

use crate::qsyntax::{self, Kind, Span};
use crate::schema::Fields;
use crate::tokenizer::query_tokens;

/// The filters of a request. Empty lists and `None` do not narrow the search.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Filter {
    /// The search box.
    pub text: String,
    /// Main log paths of the log groups.
    pub groups: Vec<String>,
    /// Unix seconds, included.
    pub start: Option<i64>,
    /// Unix seconds, excluded.
    pub end: Option<i64>,
    pub ips: Vec<String>,
    pub methods: Vec<String>,
    pub statuses: Vec<u64>,
    pub paths: Vec<String>,
    pub user_agents: Vec<String>,
    pub referers: Vec<String>,
    pub countries: Vec<String>,
    pub provinces: Vec<String>,
    pub browsers: Vec<String>,
    pub systems: Vec<String>,
    pub devices: Vec<String>,
    /// Error log levels, lower case.
    pub levels: Vec<String>,
}

fn term(field: Field, text: &str) -> Box<dyn Query> {
    Box::new(TermQuery::new(Term::from_field_text(field, text), IndexRecordOption::Basic))
}

/// Any of the values, exact.
fn any_of(field: Field, values: &[String]) -> Option<Box<dyn Query>> {
    match values {
        [] => None,
        [one] => Some(term(field, one)),
        many => Some(Box::new(BooleanQuery::union(many.iter().map(|v| term(field, v)).collect()))),
    }
}

/// The analyzed tokens as a phrase at their positions.
fn phrase(field: Field, text: &str) -> Box<dyn Query> {
    let tokens = query_tokens(text, true);
    let Some(first) = tokens.first().map(|t| t.0) else {
        return Box::new(EmptyQuery);
    };
    if tokens.len() == 1 {
        return term(field, &tokens[0].1);
    }
    Box::new(PhraseQuery::new_with_offset(
        tokens.iter().map(|(p, t)| (p - first, Term::from_field_text(field, t))).collect(),
    ))
}

fn any_phrase(field: Field, values: &[String]) -> Option<Box<dyn Query>> {
    match values {
        [] => None,
        [one] => Some(phrase(field, one)),
        many => Some(Box::new(BooleanQuery::union(many.iter().map(|v| phrase(field, v)).collect()))),
    }
}

/// The search box: every token of the text has to match the raw line.
fn text_query(field: Field, text: &str) -> Box<dyn Query> {
    let tokens = query_tokens(text, false);
    if tokens.is_empty() {
        return Box::new(EmptyQuery);
    }
    Box::new(BooleanQuery::new(tokens.iter().map(|(_, t)| (Occur::Must, term(field, t))).collect()))
}

/// The analyzed tokens of a quoted text as a phrase of the raw line.
fn raw_phrase(field: Field, text: &str) -> Box<dyn Query> {
    let tokens = query_tokens(text, false);
    let Some(first) = tokens.first().map(|t| t.0) else {
        return Box::new(EmptyQuery);
    };
    if tokens.len() == 1 {
        return term(field, &tokens[0].1);
    }
    Box::new(PhraseQuery::new_with_offset(
        tokens.iter().map(|(p, t)| (p - first, Term::from_field_text(field, t))).collect(),
    ))
}

/// Exact match of a keyword that ignores the case.
fn keyword_ci(field: Field, value: &str) -> Box<dyn Query> {
    match RegexQuery::from_pattern(&format!("(?i){}", regex::escape(value)), field) {
        Ok(q) => Box::new(q),
        Err(_) => term(field, value),
    }
}

fn bounds<T: Copy>(span: &Span<T>, make: impl Fn(T) -> Term) -> (Bound<Term>, Bound<Term>) {
    let map = |b: Bound<T>| match b {
        Bound::Included(v) => Bound::Included(make(v)),
        Bound::Excluded(v) => Bound::Excluded(make(v)),
        Bound::Unbounded => Bound::Unbounded,
    };
    (map(span.lo), map(span.hi))
}

fn range_of<T: Copy>(span: &Span<T>, make: impl Fn(T) -> Term) -> Box<dyn Query> {
    let (lo, hi) = bounds(span, make);
    Box::new(RangeQuery::new(lo, hi))
}

/// The query of one part of the search box.
fn kind_query(f: &Fields, kind: &Kind) -> Box<dyn Query> {
    match kind {
        Kind::Text(text) => text_query(f.raw, text),
        Kind::Phrase(text) => raw_phrase(f.raw, text),
        Kind::Status(span) => match (span.lo, span.hi) {
            (Bound::Included(a), Bound::Included(b)) if a == b => {
                Box::new(TermQuery::new(Term::from_field_u64(f.status, a), IndexRecordOption::Basic))
            }
            _ => range_of(span, |v| Term::from_field_u64(f.status, v)),
        },
        Kind::Method(v) => keyword_ci(f.method, v),
        Kind::Ip(lo, hi) => Box::new(RangeQuery::new(
            Bound::Included(Term::from_field_ip_addr(f.ip_addr, Ipv6Addr::from(*lo))),
            Bound::Included(Term::from_field_ip_addr(f.ip_addr, Ipv6Addr::from(*hi))),
        )),
        Kind::Path(v) => phrase(f.path, v),
        Kind::UserAgent(v) => phrase(f.user_agent, v),
        Kind::Referer(v) => phrase(f.referer, v),
        Kind::Browser(v) => keyword_ci(f.browser, v),
        Kind::Os(v) => keyword_ci(f.os, v),
        Kind::Device(v) => keyword_ci(f.device_type, v),
        Kind::Country(v) => keyword_ci(f.region_code, v),
        Kind::Region(v) => keyword_ci(f.province, v),
        Kind::City(v) => keyword_ci(f.city, v),
        Kind::Bytes(span) => range_of(span, |v| Term::from_field_u64(f.bytes_sent, v)),
        Kind::RequestTime(span) => range_of(span, |v| Term::from_field_f64(f.request_time, v)),
        Kind::Level(v) => term(f.level, v),
    }
}

/// Timestamps in `[start, end)`, either side may be open.
pub fn time_range(f: &Fields, start: Option<i64>, end: Option<i64>) -> Option<Box<dyn Query>> {
    if start.is_none() && end.is_none() {
        return None;
    }
    let bound = |v: Option<i64>, included: bool| match v {
        Some(v) if included => Bound::Included(Term::from_field_i64(f.ts, v)),
        Some(v) => Bound::Excluded(Term::from_field_i64(f.ts, v)),
        None => Bound::Unbounded,
    };
    Some(Box::new(RangeQuery::new(bound(start, true), bound(end, false))))
}

/// Builds the query of a request.
pub fn build(f: &Fields, filter: &Filter) -> Box<dyn Query> {
    let mut clauses: Vec<Box<dyn Query>> = Vec::new();
    let mut excluded: Vec<Box<dyn Query>> = Vec::new();
    for item in qsyntax::parse(&filter.text).items {
        let q = kind_query(f, &item.kind);
        if item.exclude {
            excluded.push(q);
        } else {
            clauses.push(q);
        }
    }
    clauses.extend(time_range(f, filter.start, filter.end));
    clauses.extend(any_of(f.main_log_path, &filter.groups));
    clauses.extend(any_of(f.ip, &filter.ips));
    if !filter.statuses.is_empty() {
        let terms: Vec<Box<dyn Query>> = filter
            .statuses
            .iter()
            .map(|s| {
                Box::new(TermQuery::new(Term::from_field_u64(f.status, *s), IndexRecordOption::Basic)) as Box<dyn Query>
            })
            .collect();
        clauses.push(if terms.len() == 1 {
            terms.into_iter().next().expect("one")
        } else {
            Box::new(BooleanQuery::union(terms))
        });
    }
    clauses.extend(any_of(f.method, &filter.methods));
    clauses.extend(any_of(f.region_code, &filter.countries));
    clauses.extend(any_of(f.province, &filter.provinces));
    clauses.extend(any_phrase(f.path, &filter.paths));
    clauses.extend(any_phrase(f.user_agent, &filter.user_agents));
    clauses.extend(any_phrase(f.referer, &filter.referers));
    clauses.extend(any_of(f.browser, &filter.browsers));
    clauses.extend(any_of(f.os, &filter.systems));
    clauses.extend(any_of(f.device_type, &filter.devices));
    clauses.extend(any_of(f.level, &filter.levels));

    if !excluded.is_empty() {
        let mut parts: Vec<(Occur, Box<dyn Query>)> = clauses.into_iter().map(|q| (Occur::Must, q)).collect();
        if parts.is_empty() {
            parts.push((Occur::Must, Box::new(AllQuery)));
        }
        parts.extend(excluded.into_iter().map(|q| (Occur::MustNot, q)));
        return Box::new(BooleanQuery::new(parts));
    }
    match clauses.len() {
        0 => Box::new(AllQuery),
        1 => clauses.into_iter().next().expect("one"),
        _ => Box::new(BooleanQuery::intersection(clauses)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use tantivy::collector::Count;

    /// One document of the fixture.
    struct Row {
        ts: i64,
        ip: &'static str,
        method: &'static str,
        path: &'static str,
        ua: &'static str,
        referer: &'static str,
        raw: &'static str,
        status: u64,
        bytes: u64,
        rt: Option<f64>,
        /// Browser, system, device, country and province
        client: [&'static str; 5],
    }

    const ROWS: [Row; 5] = [
        Row {
            ts: 100,
            ip: "1.1.1.1",
            method: "GET",
            path: "/wp-login.php",
            ua: "Mozilla/5.0 Chrome/126.0.0.0",
            referer: "",
            raw: "GET /wp-login.php 192.168.1.10 union%20select",
            status: 404,
            bytes: 500,
            rt: Some(0.9),
            client: ["Chrome", "Windows", "desktop", "US", "California"],
        },
        Row {
            ts: 200,
            ip: "2.2.2.2",
            method: "GET",
            path: "/about",
            ua: "curl/8.4",
            referer: "https://www.google.com/",
            raw: "GET /about from google.com",
            status: 200,
            bytes: 1500,
            rt: Some(0.1),
            client: ["", "", "", "", ""],
        },
        Row {
            ts: 300,
            ip: "1.1.1.1",
            method: "GET",
            path: "/about/us",
            ua: "Mozilla/5.0 Firefox/127.0",
            referer: "",
            raw: "GET /about/us nginx ui",
            status: 200,
            bytes: 50,
            rt: None,
            client: ["Firefox", "Linux", "desktop", "CN", "广东"],
        },
        Row {
            ts: 400,
            ip: "3.3.3.3",
            method: "POST",
            path: "/how-to/start",
            ua: "Googlebot/2.1",
            referer: "",
            raw: "POST /how-to/start",
            status: 500,
            bytes: 20_000,
            rt: Some(2.5),
            client: ["Googlebot", "", "bot", "US", ""],
        },
        Row {
            ts: 500,
            ip: "2001:db8::1",
            method: "GET",
            path: "/",
            ua: "Safari/17",
            referer: "",
            raw: "GET / ipv6",
            status: 200,
            bytes: 10,
            rt: Some(0.01),
            client: ["Safari", "iOS", "mobile", "JP", ""],
        },
    ];

    /// A small index with a few documents, to count what a filter matches.
    fn index() -> (tantivy::Index, Fields) {
        let (schema, f) = schema::build();
        let index = tantivy::Index::create_in_ram(schema);
        crate::tokenizer::register(&index);
        let mut w = index.writer_with_num_threads::<tantivy::TantivyDocument>(1, 15_000_000).unwrap();
        for r in &ROWS {
            let mut d = tantivy::TantivyDocument::default();
            d.add_i64(f.ts, r.ts);
            d.add_text(f.ip, r.ip);
            d.add_ip_addr(f.ip_addr, qsyntax::ip_to_v6(r.ip).unwrap());
            d.add_text(f.method, r.method);
            d.add_text(f.path, r.path);
            d.add_text(f.user_agent, r.ua);
            if !r.referer.is_empty() {
                d.add_text(f.referer, r.referer);
            }
            d.add_text(f.raw, r.raw);
            d.add_u64(f.status, r.status);
            d.add_u64(f.bytes_sent, r.bytes);
            if let Some(t) = r.rt {
                d.add_f64(f.request_time, t);
            }
            for (field, value) in [f.browser, f.os, f.device_type, f.region_code, f.province].into_iter().zip(r.client)
            {
                if !value.is_empty() {
                    d.add_text(field, value);
                }
            }
            d.add_text(f.main_log_path, "/l/a.log");
            d.add_text(f.fp, "x");
            d.add_u64(f.off, 0);
            w.add_document(d).unwrap();
        }
        w.commit().unwrap();
        (index, f)
    }

    fn count(index: &tantivy::Index, f: &Fields, filter: &Filter) -> usize {
        let reader = index.reader().unwrap();
        reader.searcher().search(build(f, filter).as_ref(), &Count).unwrap()
    }

    #[test]
    fn empty_filter_matches_everything() {
        let (index, f) = index();
        assert_eq!(count(&index, &f, &Filter::default()), 5);
    }

    #[test]
    fn search_box_needs_every_token() {
        let (index, f) = index();
        let with = |t: &str| Filter { text: t.into(), ..Default::default() };
        assert_eq!(count(&index, &f, &with("about")), 2);
        assert_eq!(count(&index, &f, &with("about google.com")), 1);
        assert_eq!(count(&index, &f, &with("about nope")), 0);
        // Stop words are ordinary words
        assert_eq!(count(&index, &f, &with("from")), 1);
        // Prefix of a dotted number, both forms of an escape
        assert_eq!(count(&index, &f, &with("192.168")), 1);
        assert_eq!(count(&index, &f, &with("union select")), 1);
        assert_eq!(count(&index, &f, &with("   ")), 5);
    }

    #[test]
    fn phrase_filters_match_the_tokens_in_order() {
        let (index, f) = index();
        let paths = |p: &str| Filter { paths: vec![p.into()], ..Default::default() };
        assert_eq!(count(&index, &f, &paths("/about")), 2);
        assert_eq!(count(&index, &f, &paths("/about/us")), 1);
        assert_eq!(count(&index, &f, &paths("us/about")), 0);
        assert_eq!(count(&index, &f, &paths("login.php")), 1);
        assert_eq!(count(&index, &f, &paths("/how-to/")), 1);
        let ua = Filter { user_agents: vec!["Chrome/126".into()], ..Default::default() };
        assert_eq!(count(&index, &f, &ua), 1);
        let either = Filter { paths: vec!["/about/us".into(), "/wp-login.php".into()], ..Default::default() };
        assert_eq!(count(&index, &f, &either), 2);
        assert_eq!(count(&index, &f, &paths("!!!")), 0);
    }

    #[test]
    fn time_range_includes_the_start_and_excludes_the_end() {
        let (index, f) = index();
        let range = |s, e| Filter { start: s, end: e, ..Default::default() };
        assert_eq!(count(&index, &f, &range(Some(200), Some(400))), 2);
        assert_eq!(count(&index, &f, &range(Some(200), Some(401))), 3);
        assert_eq!(count(&index, &f, &range(Some(201), Some(400))), 1);
        assert_eq!(count(&index, &f, &range(None, Some(200))), 1);
        assert_eq!(count(&index, &f, &range(Some(300), None)), 3);
    }

    #[test]
    fn exact_filters_combine_with_and() {
        let (index, f) = index();
        let filter = Filter { ips: vec!["1.1.1.1".into()], statuses: vec![200, 404], ..Default::default() };
        assert_eq!(count(&index, &f, &filter), 2);
        let filter = Filter { ips: vec!["1.1.1.1".into()], statuses: vec![404], ..Default::default() };
        assert_eq!(count(&index, &f, &filter), 1);
        let filter = Filter { groups: vec!["/l/other.log".into()], ..Default::default() };
        assert_eq!(count(&index, &f, &filter), 0);
        let filter = Filter { groups: vec!["/l/a.log".into()], text: "about".into(), ..Default::default() };
        assert_eq!(count(&index, &f, &filter), 2);
    }

    /// Matching timestamps of a search box text, to compare whole result sets.
    fn hits(index: &tantivy::Index, f: &Fields, text: &str) -> Vec<i64> {
        use tantivy::collector::DocSetCollector;
        use tantivy::columnar::Column;
        let searcher = index.reader().unwrap().searcher();
        let filter = Filter { text: text.into(), ..Default::default() };
        let docs = searcher.search(build(f, &filter).as_ref(), &DocSetCollector).unwrap();
        let mut out: Vec<i64> = docs
            .into_iter()
            .map(|a| {
                let col: Column<i64> = searcher.segment_reader(a.segment_ord).fast_fields().i64("ts").unwrap();
                col.first(a.doc_id).unwrap()
            })
            .collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn field_syntax_table() {
        let (index, f) = index();
        // (search box, timestamps of the documents that match)
        let table: &[(&str, &[i64])] = &[
            ("status:404", &[100]),
            ("status:5xx", &[400]),
            ("status:2xx", &[200, 300, 500]),
            ("status:400-499", &[100]),
            ("status:400..599", &[100, 400]),
            ("status:>=404", &[100, 400]),
            ("status:>404", &[400]),
            ("status:<404", &[200, 300, 500]),
            ("status:<=200", &[200, 300, 500]),
            ("method:POST", &[400]),
            ("method:post", &[400]),
            ("method:get", &[100, 200, 300, 500]),
            ("ip:1.1.1.1", &[100, 300]),
            ("ip:1.1.1.0/24", &[100, 300]),
            ("ip:1.0.0.0/8", &[100, 300]),
            ("ip:0.0.0.0/0", &[100, 200, 300, 400]),
            ("ip:2.2.2.2/32", &[200]),
            ("ip:10.0.0.0/8", &[]),
            ("ip:2001:db8::/32", &[500]),
            ("ip:2001:db8::1", &[500]),
            ("ip:2001:db9::/32", &[]),
            ("ip:::/0", &[100, 200, 300, 400, 500]),
            ("path:/about", &[200, 300]),
            ("path:/about/us", &[300]),
            ("path:us/about", &[]),
            ("path:\"/how-to/\"", &[400]),
            ("ua:curl", &[200]),
            ("ua:Chrome/126", &[100]),
            ("referer:google", &[200]),
            ("browser:chrome", &[100]),
            ("browser:Firefox", &[300]),
            ("os:ios", &[500]),
            ("device:desktop", &[100, 300]),
            ("country:cn", &[300]),
            ("country:US", &[100, 400]),
            ("region:广东", &[300]),
            ("region:california", &[100]),
            ("bytes:>1000", &[200, 400]),
            ("bytes:>=1500", &[200, 400]),
            ("bytes:<50", &[500]),
            ("bytes:50..500", &[100, 300]),
            ("bytes:10", &[500]),
            ("rt:>0.5", &[100, 400]),
            ("rt:<=0.1", &[200, 500]),
            ("rt:0.1..1", &[100, 200]),
            ("rt:<0.05", &[500]),
            // Combined with AND
            ("status:200 method:get path:/about", &[200, 300]),
            ("status:200 bytes:>1000", &[200]),
            ("status:5xx ip:3.3.3.3 rt:>2", &[400]),
            ("about status:200", &[200, 300]),
            ("status:404 about", &[]),
            // Excluding
            ("-about", &[100, 400, 500]),
            ("-status:200", &[100, 400]),
            ("about -google.com", &[300]),
            ("status:200 -path:/about", &[500]),
            ("-ip:1.0.0.0/8 -status:5xx", &[200, 500]),
            ("-nothinglikethis", &[100, 200, 300, 400, 500]),
            ("-\"union select\"", &[200, 300, 400, 500]),
            // Phrases of the raw line
            ("\"union select\"", &[100]),
            ("\"select union\"", &[]),
            ("\"get /about\"", &[200, 300]),
            ("\"get /about/us\"", &[300]),
            ("\"/about nginx\"", &[]),
            ("\"192.168.1.10 union\"", &[100]),
            ("\"google.com\"", &[200]),
            // Text that is not a filter
            ("status:abc", &[]),
            ("color:red", &[]),
            ("bytes:1k", &[]),
            ("ip:1.2.3", &[]),
        ];
        for (text, want) in table {
            assert_eq!(hits(&index, &f, text), *want, "search box {text:?}");
        }
    }

    #[test]
    fn a_quoted_text_matches_its_words_in_order() {
        let (index, f) = index();
        let count_of = |text: &str| count(&index, &f, &Filter { text: text.into(), ..Default::default() });
        assert_eq!(count_of("\"union select\""), 1);
        assert_eq!(count_of("\"select union\""), 0);
        assert_eq!(count_of("\"union nothing\""), 0);
        assert_eq!(count_of("-\"union select\""), 4);
    }

    #[test]
    fn numbers_and_dotted_words_inside_phrases() {
        let (index, f) = index();
        assert_eq!(hits(&index, &f, "\"wp-login.php 192.168\""), [100]);
        assert_eq!(hits(&index, &f, "\"wp-login.php 192.168.1.10\""), [100]);
        assert!(hits(&index, &f, "\"wp-login.php 10.0\"").is_empty());
    }

    #[test]
    fn error_entries_match_by_level_and_text() {
        let (schema, f) = schema::build();
        let index = tantivy::Index::create_in_ram(schema);
        crate::tokenizer::register(&index);
        let mut w = index.writer_with_num_threads::<tantivy::TantivyDocument>(1, 15_000_000).unwrap();
        let mut times = crate::parse::TimeCache::default();
        for line in [
            "2026/09/07 15:32:29 [error] 1#1: *1 open() failed, client: 1.1.1.1, server: a, request: \"GET /x HTTP/1.1\"",
            "2026/09/07 15:32:30 [warn] 1#1: *2 upstream response is buffered, client: 2.2.2.2, server: a",
            "2026/09/07 15:32:31 [notice] 1#1: signal process started",
        ] {
            let e = crate::parse::parse_error_line(line, &mut times).unwrap();
            w.add_document(crate::pipeline::build_error_doc(&f, &e, line, "/l/error.log", "x", 0)).unwrap();
        }
        w.commit().unwrap();
        let text = |t: &str| count(&index, &f, &Filter { text: t.into(), ..Default::default() });
        assert_eq!(text("level:error"), 1);
        assert_eq!(text("level:WARNING"), 1);
        assert_eq!(text("-level:notice"), 2);
        assert_eq!(text("ip:1.1.1.0/24"), 1);
        assert_eq!(text("path:/x"), 1);
        assert_eq!(text("buffered"), 1);
        let levels = |l: &[&str]| {
            count(&index, &f, &Filter { levels: l.iter().map(|s| (*s).to_owned()).collect(), ..Default::default() })
        };
        assert_eq!(levels(&["error", "warn"]), 2);
    }
}
