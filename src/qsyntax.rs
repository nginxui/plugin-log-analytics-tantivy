//! The syntax of the search box.
//!
//! Words and `field:value` filters are separated by spaces and combine with
//! AND. A leading `-` excludes, a quoted text is a phrase. Whatever does not
//! parse stays ordinary text, the parser never fails. Tokens that looked like
//! a filter but were not one are reported as warnings.
//!
//! | Filter | Meaning |
//! | --- | --- |
//! | `status:404`, `status:5xx`, `status:400-499`, `status:>=500` | status code |
//! | `method:POST` | request method |
//! | `ip:1.2.3.4`, `ip:10.0.0.0/8`, `ip:2001:db8::/32` | client address |
//! | `path:/api/`, `ua:curl`, `referer:google` | words of the field, in order |
//! | `browser:`, `os:`, `device:` | detected client |
//! | `country:`, `region:`, `city:` | location |
//! | `bytes:>1000`, `rt:>0.5`, `rt:0.1..0.5` | sent bytes, request time |

use std::net::{IpAddr, Ipv6Addr};
use std::ops::Bound;

use serde::Serialize;

/// Lower and upper bound of a numeric filter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span<T> {
    pub lo: Bound<T>,
    pub hi: Bound<T>,
}

impl<T: Copy> Span<T> {
    fn exactly(v: T) -> Self {
        Span { lo: Bound::Included(v), hi: Bound::Included(v) }
    }

    fn between(lo: T, hi: T) -> Self {
        Span { lo: Bound::Included(lo), hi: Bound::Included(hi) }
    }
}

/// What one part of the search box asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// Words, matched as tokens of the raw line.
    Text(String),
    /// A quoted text, matched as a phrase of the raw line when that is indexed.
    Phrase(String),
    Status(Span<u64>),
    Method(String),
    /// Inclusive range of the address as a number, IPv4 mapped into IPv6.
    Ip(u128, u128),
    Path(String),
    UserAgent(String),
    Referer(String),
    Browser(String),
    Os(String),
    Device(String),
    Country(String),
    Region(String),
    City(String),
    Bytes(Span<u64>),
    RequestTime(Span<f64>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub exclude: bool,
    pub kind: Kind,
}

/// A token that was taken as text although it looks like something else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub token: String,
    /// `unknown_field`, `invalid_value` or `unterminated_quote`.
    pub reason: &'static str,
}

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub items: Vec<Item>,
    pub warnings: Vec<Warning>,
}

/// The field names of the syntax, with their aliases.
const FIELDS: [&str; 18] = [
    "status",
    "method",
    "ip",
    "path",
    "ua",
    "user_agent",
    "referer",
    "referrer",
    "browser",
    "os",
    "device",
    "country",
    "region",
    "city",
    "bytes",
    "rt",
    "request_time",
    "time",
];

/// The IPv6 form of an address text: IPv4 is mapped into IPv6.
pub fn ip_to_v6(text: &str) -> Option<Ipv6Addr> {
    match text.trim().parse::<IpAddr>().ok()? {
        IpAddr::V4(v4) => Some(v4.to_ipv6_mapped()),
        IpAddr::V6(v6) => Some(v6),
    }
}

/// Parses `value` as a number with an optional comparison or a range `a..b`.
/// Either side of a range may be left open.
fn span_of<T>(value: &str, valid: impl Fn(&T) -> bool) -> Option<Span<T>>
where
    T: std::str::FromStr + PartialOrd + Copy,
{
    let number = |s: &str| s.trim().parse::<T>().ok().filter(|v| valid(v));
    for (prefix, lower) in [(">=", true), (">", true), ("<=", false), ("<", false)] {
        if let Some(rest) = value.strip_prefix(prefix) {
            let v = number(rest)?;
            let included = prefix.ends_with('=');
            let bound = if included { Bound::Included(v) } else { Bound::Excluded(v) };
            return Some(if lower {
                Span { lo: bound, hi: Bound::Unbounded }
            } else {
                Span { lo: Bound::Unbounded, hi: bound }
            });
        }
    }
    if let Some((a, b)) = value.split_once("..") {
        let (lo, hi) = (number(a), number(b));
        return match (a.is_empty(), b.is_empty(), lo, hi) {
            (false, false, Some(lo), Some(hi)) if lo <= hi => Some(Span::between(lo, hi)),
            (false, true, Some(lo), _) => Some(Span { lo: Bound::Included(lo), hi: Bound::Unbounded }),
            (true, false, _, Some(hi)) => Some(Span { lo: Bound::Unbounded, hi: Bound::Included(hi) }),
            _ => None,
        };
    }
    number(value).map(Span::exactly)
}

/// A status filter: a code, a class such as `5xx`, `a-b`, or the forms of [`span_of`].
fn status_span(value: &str) -> Option<Span<u64>> {
    let b = value.as_bytes();
    if b.len() == 3 && b[0].is_ascii_digit() && b[0] != b'0' && b[1..].eq_ignore_ascii_case(b"xx") {
        let base = u64::from(b[0] - b'0') * 100;
        return Some(Span::between(base, base + 99));
    }
    if let Some((a, z)) = value.split_once('-') {
        let (lo, hi) = (a.trim().parse::<u64>().ok()?, z.trim().parse::<u64>().ok()?);
        return (lo <= hi).then(|| Span::between(lo, hi));
    }
    span_of(value, |_| true)
}

/// An address or a network as the range of numbers it covers.
fn ip_range(value: &str) -> Option<(u128, u128)> {
    let (addr, len) = match value.split_once('/') {
        Some((a, l)) => (a, Some(l.parse::<u32>().ok()?)),
        None => (value, None),
    };
    let ip = addr.parse::<IpAddr>().ok()?;
    let (v6, bits) = match ip {
        IpAddr::V4(v4) => (v4.to_ipv6_mapped(), len.map_or(Some(128), |l| (l <= 32).then_some(l + 96))?),
        IpAddr::V6(v6) => (v6, len.map_or(Some(128), |l| (l <= 128).then_some(l))?),
    };
    let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
    let lo = u128::from(v6) & mask;
    Some((lo, lo | !mask))
}

/// The filter a `name:value` token stands for, `None` when the value is malformed.
fn field_kind(name: &str, value: &str) -> Option<Kind> {
    let text = |make: fn(String) -> Kind| (!value.is_empty()).then(|| make(value.to_owned()));
    match name {
        "status" => status_span(value).map(Kind::Status),
        "method" => text(Kind::Method),
        "ip" => ip_range(value).map(|(lo, hi)| Kind::Ip(lo, hi)),
        "path" => text(Kind::Path),
        "ua" | "user_agent" => text(Kind::UserAgent),
        "referer" | "referrer" => text(Kind::Referer),
        "browser" => text(Kind::Browser),
        "os" => text(Kind::Os),
        "device" => text(Kind::Device),
        "country" => text(Kind::Country),
        "region" => text(Kind::Region),
        "city" => text(Kind::City),
        "bytes" => span_of::<u64>(value, |_| true).map(Kind::Bytes),
        "rt" | "request_time" | "time" => span_of::<f64>(value, |v| v.is_finite() && *v >= 0.0).map(Kind::RequestTime),
        _ => None,
    }
}

fn is_quoted(s: &str) -> bool {
    s.len() >= 3 && s.starts_with('"') && s.ends_with('"') && !s[1..s.len() - 1].contains('"')
}

fn unquote(s: &str) -> &str {
    if is_quoted(s) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// End of the token that starts at `from`. Quotes keep spaces together when
/// `quotes` is set. The flag tells whether a quote was left open.
fn token_end(s: &str, from: usize, quotes: bool) -> (usize, bool) {
    let mut open = false;
    for (i, c) in s[from..].char_indices() {
        if c == '"' && quotes {
            open = !open;
        } else if c.is_whitespace() && !open {
            return (from + i, false);
        }
    }
    (s.len(), open)
}

/// The most parts of the search box that are read, the rest is ignored.
pub const MAX_TOKENS: usize = 64;

/// Splits the search box into parts and reads them.
pub fn parse(input: &str) -> Parsed {
    let mut out = Parsed::default();
    let mut words: Vec<&str> = Vec::new();
    let mut tokens = 0;
    let mut i = 0;
    while i < input.len() {
        let c = input[i..].chars().next().expect("a character");
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        if tokens == MAX_TOKENS {
            out.warnings.push(Warning { token: input[i..].chars().take(32).collect(), reason: "too_many_terms" });
            break;
        }
        tokens += 1;
        let (mut end, open) = token_end(input, i, true);
        if open {
            // A lone quote is ordinary text
            end = token_end(input, i, false).0;
            out.warnings.push(Warning { token: input[i..end].to_owned(), reason: "unterminated_quote" });
        }
        let token = &input[i..end];
        i = end;

        // A dash that is followed by more text excludes
        let (exclude, body) = match token.strip_prefix('-') {
            Some(rest) if !rest.is_empty() => (true, rest),
            _ => (false, token),
        };
        let quoted_whole = is_quoted(body);
        if quoted_whole {
            out.items.push(Item { exclude, kind: Kind::Phrase(unquote(body).to_owned()) });
            continue;
        }
        if let Some((name, value)) = body.split_once(':') {
            let lower = name.to_ascii_lowercase();
            if FIELDS.contains(&lower.as_str()) && !value.is_empty() {
                match field_kind(&lower, unquote(value)) {
                    Some(kind) => out.items.push(Item { exclude, kind }),
                    None => {
                        out.warnings.push(Warning { token: token.to_owned(), reason: "invalid_value" });
                        push_text(&mut out, &mut words, exclude, body);
                    }
                }
                continue;
            }
            // Letters only, and a value that is not a URL or an address
            let looks_like_field = name.len() >= 2
                && name.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
                && !value.is_empty()
                && !value.starts_with('/')
                && !value.starts_with(':');
            if looks_like_field && !FIELDS.contains(&lower.as_str()) {
                out.warnings.push(Warning { token: token.to_owned(), reason: "unknown_field" });
            }
        }
        push_text(&mut out, &mut words, exclude, body);
    }
    if !words.is_empty() {
        out.items.push(Item { exclude: false, kind: Kind::Text(words.join(" ")) });
    }
    out
}

/// Plain words are collected into one text, excluded words stand alone.
fn push_text<'a>(out: &mut Parsed, words: &mut Vec<&'a str>, exclude: bool, body: &'a str) {
    if exclude {
        out.items.push(Item { exclude: true, kind: Kind::Text(body.to_owned()) });
    } else {
        words.push(body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Kind {
        Kind::Text(s.to_owned())
    }

    fn inc(kind: Kind) -> Item {
        Item { exclude: false, kind }
    }

    fn exc(kind: Kind) -> Item {
        Item { exclude: true, kind }
    }

    fn items(input: &str) -> Vec<Item> {
        parse(input).items
    }

    fn v4(a: u8, b: u8, c: u8, d: u8) -> u128 {
        u128::from(std::net::Ipv4Addr::new(a, b, c, d).to_ipv6_mapped())
    }

    #[test]
    fn plain_words_stay_one_text() {
        assert_eq!(items("about google.com"), [inc(text("about google.com"))]);
        assert_eq!(items("  spaced   out "), [inc(text("spaced out"))]);
        assert!(items("").is_empty());
        assert!(items("   ").is_empty());
        // A dash inside a word does not exclude
        assert_eq!(items("wp-login.php"), [inc(text("wp-login.php"))]);
        // A lone dash is text
        assert_eq!(items("-"), [inc(text("-"))]);
    }

    #[test]
    fn status_forms() {
        use Bound::*;
        let cases: &[(&str, Span<u64>)] = &[
            ("status:404", Span::exactly(404)),
            ("status:5xx", Span::between(500, 599)),
            ("status:4XX", Span::between(400, 499)),
            ("status:400-499", Span::between(400, 499)),
            ("status:400..499", Span::between(400, 499)),
            ("status:>=500", Span { lo: Included(500), hi: Unbounded }),
            ("status:>499", Span { lo: Excluded(499), hi: Unbounded }),
            ("status:<400", Span { lo: Unbounded, hi: Excluded(400) }),
            ("status:<=304", Span { lo: Unbounded, hi: Included(304) }),
            ("status:404..", Span { lo: Included(404), hi: Unbounded }),
            ("status:..299", Span { lo: Unbounded, hi: Included(299) }),
        ];
        for (input, span) in cases {
            assert_eq!(items(input), [inc(Kind::Status(*span))], "{input}");
        }
    }

    #[test]
    fn malformed_values_become_text_with_a_warning() {
        let cases = [
            "status:abc",
            "status:0xx",
            "status:600-500",
            "status:>",
            "bytes:1k",
            "bytes:>x",
            "rt:fast",
            "rt:>-1",
            "rt:nan",
            "ip:1.2.3",
            "ip:1.2.3.4/33",
            "ip:2001:db8::/129",
            "ip:10.0.0.0/x",
        ];
        for input in cases {
            let parsed = parse(input);
            assert_eq!(parsed.items, [inc(text(input))], "{input}");
            assert_eq!(parsed.warnings, [Warning { token: input.into(), reason: "invalid_value" }], "{input}");
        }
    }

    #[test]
    fn unknown_fields_are_text_and_warn_only_when_they_look_like_filters() {
        let parsed = parse("color:red");
        assert_eq!(parsed.items, [inc(text("color:red"))]);
        assert_eq!(parsed.warnings, [Warning { token: "color:red".into(), reason: "unknown_field" }]);
        for quiet in ["http://example.com/a", "fe80::1", "2001:db8::1", "12:30", "a:/b", "status:", "x:y"] {
            assert!(parse(quiet).warnings.is_empty(), "{quiet}");
            assert_eq!(parse(quiet).items, [inc(text(quiet))], "{quiet}");
        }
    }

    #[test]
    fn methods_and_text_fields() {
        assert_eq!(items("method:POST"), [inc(Kind::Method("POST".into()))]);
        assert_eq!(items("METHOD:get"), [inc(Kind::Method("get".into()))]);
        assert_eq!(items("path:/api/"), [inc(Kind::Path("/api/".into()))]);
        assert_eq!(items("ua:curl"), [inc(Kind::UserAgent("curl".into()))]);
        assert_eq!(items("user_agent:curl"), [inc(Kind::UserAgent("curl".into()))]);
        assert_eq!(items("referer:google"), [inc(Kind::Referer("google".into()))]);
        assert_eq!(items("browser:Chrome"), [inc(Kind::Browser("Chrome".into()))]);
        assert_eq!(
            items("os:macOS device:mobile"),
            [inc(Kind::Os("macOS".into())), inc(Kind::Device("mobile".into()))]
        );
        assert_eq!(
            items("country:CN region:广东 city:深圳"),
            [inc(Kind::Country("CN".into())), inc(Kind::Region("广东".into())), inc(Kind::City("深圳".into())),]
        );
        assert_eq!(items("path:\"/a b\""), [inc(Kind::Path("/a b".into()))]);
    }

    #[test]
    fn addresses_and_networks() {
        assert_eq!(items("ip:1.2.3.4"), [inc(Kind::Ip(v4(1, 2, 3, 4), v4(1, 2, 3, 4)))]);
        assert_eq!(items("ip:192.168.0.0/16"), [inc(Kind::Ip(v4(192, 168, 0, 0), v4(192, 168, 255, 255)))]);
        assert_eq!(items("ip:10.1.2.3/8"), [inc(Kind::Ip(v4(10, 0, 0, 0), v4(10, 255, 255, 255)))]);
        assert_eq!(items("ip:0.0.0.0/0"), [inc(Kind::Ip(v4(0, 0, 0, 0), v4(255, 255, 255, 255)))]);
        let lo = u128::from("2001:db8::".parse::<Ipv6Addr>().unwrap());
        let hi = u128::from("2001:db8:ffff:ffff:ffff:ffff:ffff:ffff".parse::<Ipv6Addr>().unwrap());
        assert_eq!(items("ip:2001:db8::/32"), [inc(Kind::Ip(lo, hi))]);
        let one = u128::from("::1".parse::<Ipv6Addr>().unwrap());
        assert_eq!(items("ip:::1"), [inc(Kind::Ip(one, one))]);
        assert_eq!(items("ip:::/0"), [inc(Kind::Ip(0, u128::MAX))]);
    }

    #[test]
    fn numbers_with_comparisons_and_ranges() {
        use Bound::*;
        assert_eq!(items("bytes:>1000"), [inc(Kind::Bytes(Span { lo: Excluded(1000), hi: Unbounded }))]);
        assert_eq!(items("bytes:1000..5000"), [inc(Kind::Bytes(Span::between(1000, 5000)))]);
        assert_eq!(items("bytes:0"), [inc(Kind::Bytes(Span::exactly(0)))]);
        assert_eq!(items("rt:>0.5"), [inc(Kind::RequestTime(Span { lo: Excluded(0.5), hi: Unbounded }))]);
        assert_eq!(items("rt:<=2"), [inc(Kind::RequestTime(Span { lo: Unbounded, hi: Included(2.0) }))]);
        assert_eq!(items("rt:0.1..0.5"), [inc(Kind::RequestTime(Span::between(0.1, 0.5)))]);
        assert_eq!(items("request_time:>1"), [inc(Kind::RequestTime(Span { lo: Excluded(1.0), hi: Unbounded }))]);
        // A reversed range is malformed
        assert_eq!(parse("bytes:5..1").warnings.len(), 1);
    }

    #[test]
    fn a_dash_excludes() {
        assert_eq!(items("-bot"), [exc(text("bot"))]);
        assert_eq!(items("about -bot"), [exc(text("bot")), inc(text("about"))]);
        assert_eq!(items("-status:5xx"), [exc(Kind::Status(Span::between(500, 599)))]);
        assert_eq!(items("-ip:10.0.0.0/8"), [exc(Kind::Ip(v4(10, 0, 0, 0), v4(10, 255, 255, 255)))]);
        assert_eq!(items("-\"union select\""), [exc(Kind::Phrase("union select".into()))]);
        // The text of an invalid filter is excluded as text
        assert_eq!(items("-status:abc"), [exc(text("status:abc"))]);
    }

    #[test]
    fn quoted_phrases() {
        assert_eq!(items("\"union select\""), [inc(Kind::Phrase("union select".into()))]);
        assert_eq!(items("a \"b c\" d"), [inc(Kind::Phrase("b c".into())), inc(text("a d"))]);
        assert_eq!(items("\"one\""), [inc(Kind::Phrase("one".into()))]);
        assert_eq!(items("\"\""), [inc(text("\"\""))]);
    }

    #[test]
    fn a_lone_quote_is_text_with_a_warning() {
        let parsed = parse("\"open phrase");
        assert_eq!(parsed.items, [inc(text("\"open phrase"))]);
        assert_eq!(parsed.warnings, [Warning { token: "\"open".into(), reason: "unterminated_quote" }]);
        // The rest of the input is still read
        let parsed = parse("a\"b status:404");
        assert_eq!(parsed.items, [inc(Kind::Status(Span::exactly(404))), inc(text("a\"b"))]);
    }

    #[test]
    fn the_examples_of_the_help_are_valid_filters() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/webapp/src/views/structured/components/search-syntax.ts");
        let Ok(source) = std::fs::read_to_string(path) else { return };
        let list = regex::Regex::new(r"examples: \[([^\]]*)\]").unwrap();
        let quoted = regex::Regex::new(r"'([^']*)'").unwrap();
        let mut seen = 0;
        for row in list.captures_iter(&source) {
            for example in quoted.captures_iter(&row[1]) {
                let parsed = parse(&example[1]);
                assert!(parsed.warnings.is_empty(), "{}: {:?}", &example[1], parsed.warnings);
                assert!(!parsed.items.is_empty(), "{} has no effect", &example[1]);
                seen += 1;
            }
        }
        assert!(seen >= 15, "found {seen} examples");
    }

    #[test]
    fn filters_combine() {
        assert_eq!(
            items("status:5xx method:POST path:/api/ -ua:curl slow"),
            [
                inc(Kind::Status(Span::between(500, 599))),
                inc(Kind::Method("POST".into())),
                inc(Kind::Path("/api/".into())),
                exc(Kind::UserAgent("curl".into())),
                inc(text("slow")),
            ]
        );
    }

    #[test]
    fn only_the_first_parts_are_read() {
        let input = (0..MAX_TOKENS + 10).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ");
        let parsed = parse(&input);
        let Kind::Text(text) = &parsed.items[0].kind else { panic!("text expected") };
        assert_eq!(text.split(' ').count(), MAX_TOKENS);
        assert_eq!(parsed.warnings.last().map(|w| w.reason), Some("too_many_terms"));
    }
}
