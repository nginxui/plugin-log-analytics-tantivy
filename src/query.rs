//! Turns the filters of a request into a tantivy query.
//!
//! The search box is analyzed like the raw line and every token has to match.
//! The path, user agent and referer filters are phrases over their analyzed
//! field. The time range includes its start and excludes its end.

use std::ops::Bound;

use tantivy::query::{AllQuery, BooleanQuery, EmptyQuery, Occur, PhraseQuery, Query, RangeQuery, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, Term};

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
    if !filter.text.trim().is_empty() {
        clauses.push(text_query(f.raw, &filter.text));
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
    use tantivy::doc;

    /// A small index with a few documents, to count what a filter matches.
    fn index() -> (tantivy::Index, Fields) {
        let (schema, f) = schema::build();
        let index = tantivy::Index::create_in_ram(schema);
        crate::tokenizer::register(&index);
        let mut w = index.writer_with_num_threads::<tantivy::TantivyDocument>(1, 15_000_000).unwrap();
        let rows: &[(i64, &str, &str, &str, &str, u64)] = &[
            (
                100,
                "1.1.1.1",
                "/wp-login.php",
                "Mozilla/5.0 Chrome/126.0.0.0",
                "GET /wp-login.php 192.168.1.10 union%20select",
                404,
            ),
            (200, "2.2.2.2", "/about", "curl/8.4", "GET /about from google.com", 200),
            (300, "1.1.1.1", "/about/us", "Mozilla/5.0 Firefox/127.0", "GET /about/us nginx ui", 200),
            (400, "3.3.3.3", "/how-to/start", "Googlebot/2.1", "POST /how-to/start", 500),
        ];
        for (ts, ip, path, ua, raw, status) in rows {
            w.add_document(doc!(
                f.ts => *ts, f.ip => *ip, f.path => *path, f.user_agent => *ua, f.raw => *raw,
                f.status => *status, f.main_log_path => "/l/a.log", f.bytes_sent => 1u64, f.fp => "x", f.off => 0u64
            ))
            .unwrap();
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
        assert_eq!(count(&index, &f, &Filter::default()), 4);
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
        assert_eq!(count(&index, &f, &with("   ")), 4);
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
        assert_eq!(count(&index, &f, &range(Some(300), None)), 2);
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
}
