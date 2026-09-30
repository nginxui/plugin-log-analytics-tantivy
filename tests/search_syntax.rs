//! The search box syntax through the API, on the varied test log.

mod common;

use common::*;
use serde_json::{json, Value};

const LINES: usize = 600;

async fn search(api: &Api, query: &str) -> Value {
    let (code, body) = api.post("/search", json!({"query": query, "limit": 5, "log_path": api.group()})).await;
    assert_eq!(code, 200, "{body}");
    body
}

async fn total(api: &Api, query: &str) -> u64 {
    search(api, query).await["total"].as_u64().unwrap()
}

/// Lines of the test log that satisfy a condition on their number.
fn expected(f: impl Fn(usize) -> bool) -> u64 {
    (0..LINES).filter(|n| f(*n)).count() as u64
}

#[tokio::test]
async fn field_filters_follow_the_fixture() {
    let api = Api::new(LINES).await;
    // Line n: client n % 5 (8.8.8.8, 1.1.1.1, 9.9.9.9, 203.0.113.7, 46.4.0.1),
    // status n % 5 (200 200 404 200 500), path (n / 2) % 5 (/, /about, /wp-login.php,
    // /api/items, /how-to/start), agent n % 4 (Chrome, iPhone, curl, Googlebot),
    // bytes 100 + n, request time 0.0(n % 9 + 1)
    let table: Vec<(&str, u64)> = vec![
        ("status:404", expected(|n| n % 5 == 2)),
        ("status:5xx", expected(|n| n % 5 == 4)),
        ("status:2xx", expected(|n| matches!(n % 5, 0 | 1 | 3))),
        ("status:400-499", expected(|n| n % 5 == 2)),
        ("status:>=404", expected(|n| matches!(n % 5, 2 | 4))),
        ("-status:200", expected(|n| matches!(n % 5, 2 | 4))),
        ("method:get", LINES as u64),
        ("method:POST", 0),
        ("ip:8.8.8.8", expected(|n| n % 5 == 0)),
        ("ip:8.8.0.0/16", expected(|n| n % 5 == 0)),
        ("ip:1.0.0.0/8", expected(|n| n % 5 == 1)),
        ("ip:203.0.113.0/24", expected(|n| n % 5 == 3)),
        ("ip:0.0.0.0/0", LINES as u64),
        ("ip:2001:db8::/32", 0),
        ("-ip:0.0.0.0/1", expected(|n| n % 5 == 3)),
        ("path:/about", expected(|n| (n / 2) % 5 == 1)),
        ("path:/api/", expected(|n| (n / 2) % 5 == 3)),
        ("path:/how-to/start status:5xx", expected(|n| (n / 2) % 5 == 4 && n % 5 == 4)),
        ("ua:curl", expected(|n| n % 4 == 2)),
        ("ua:googlebot -status:200", expected(|n| n % 4 == 3 && matches!(n % 5, 2 | 4))),
        ("browser:chrome", expected(|n| n % 4 == 0)),
        ("referer:example", LINES as u64),
        ("bytes:>=600", expected(|n| 100 + n >= 600)),
        ("bytes:<=109", expected(|n| 100 + n <= 109)),
        ("bytes:200..299", expected(|n| (200..=299).contains(&(100 + n)))),
        ("rt:>0.05", expected(|n| n % 9 + 1 > 5)),
        ("rt:0.02..0.04", expected(|n| (2..=4).contains(&(n % 9 + 1)))),
        ("wp-login.php -status:404", expected(|n| (n / 2) % 5 == 2 && n % 5 != 2)),
        ("\"GET /about\"", expected(|n| (n / 2) % 5 == 1)),
    ];
    for (query, want) in table {
        assert_eq!(total(&api, query).await, want, "query {query:?}");
    }
}

#[tokio::test]
async fn unknown_fields_and_bad_values_are_text_with_hints() {
    let api = Api::new(LINES).await;
    let body = search(&api, "status:200").await;
    assert!(body.get("query_warnings").is_none());

    let body = search(&api, "status:abc").await;
    assert_eq!(body["total"], 0);
    assert_eq!(body["query_warnings"], json!([{"token": "status:abc", "reason": "invalid_value"}]));

    let body = search(&api, "color:red").await;
    assert_eq!(body["total"], 0);
    assert_eq!(body["query_warnings"][0]["reason"], "unknown_field");

    // A URL is plain text and needs no hint
    let body = search(&api, "https://example.com/").await;
    assert!(body.get("query_warnings").is_none());
    assert!(body["total"].as_u64().unwrap() > 0);

    // The filters of the request body still work next to the syntax
    let (_, body) =
        api.post("/search", json!({"query": "status:5xx", "status": [500, 404], "method": "GET", "limit": 1})).await;
    assert_eq!(body["total"], expected(|n| n % 5 == 4));
}

#[tokio::test]
async fn quoted_text_matches_its_words_in_order() {
    let api = Api::new(200).await;
    let wp = (0..200).filter(|n| (n / 2) % 5 == 2).count() as u64;
    assert_eq!(total(&api, "\"wp login php\"").await, wp);
    assert_eq!(total(&api, "\"php wp\"").await, 0);
    assert_eq!(total(&api, "-\"wp login php\"").await, 200 - wp);
}
