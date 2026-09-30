//! Access log line parser for the combined format with two trailing time
//! fields. The rules follow the existing Go plugin.

/// One parsed log line. Text fields borrow from the line.
#[derive(Debug, Default, Clone)]
pub struct Entry<'a> {
    pub ts: i64,
    pub ip: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    pub protocol: &'a str,
    pub status: u64,
    pub bytes_sent: u64,
    pub referer: &'a str,
    pub user_agent: &'a str,
    pub request_time: Option<f64>,
    pub upstream_time: Option<f64>,
}

/// Longest line the parser accepts, longer lines are skipped.
pub const MAX_LINE_LEN: usize = 16 * 1024;

const METHODS: &[&str] = &[
    "GET",
    "POST",
    "PUT",
    "DELETE",
    "HEAD",
    "OPTIONS",
    "PATCH",
    "CONNECT",
    "TRACE",
    "PROPFIND",
    "PROPPATCH",
    "MKCOL",
    "COPY",
    "MOVE",
    "LOCK",
    "UNLOCK",
];

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn month_number(name: &str) -> Option<i64> {
    Some(match name {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

fn num(s: &str, r: std::ops::Range<usize>) -> Option<i64> {
    let part = s.get(r)?;
    if part.bytes().all(|b| b.is_ascii_digit()) {
        part.parse().ok()
    } else {
        None
    }
}

/// Parses `07/Sep/2026:15:32:29 +0000`, with or without the zone.
fn parse_nginx_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[2] != b'/' || b[6] != b'/' || b[11] != b':' {
        return None;
    }
    let day = num(s, 0..2)?;
    let month = month_number(s.get(3..6)?)?;
    let year = num(s, 7..11)?;
    let (h, mi, sec) = (num(s, 12..14)?, num(s, 15..17)?, num(s, 18..20)?);
    let mut offset = 0;
    if b.len() >= 26 && (b[21] == b'+' || b[21] == b'-') {
        let o = num(s, 22..24)? * 3600 + num(s, 24..26)? * 60;
        offset = if b[21] == b'-' { -o } else { o };
    }
    Some(days_from_civil(year, month, day) * 86400 + h * 3600 + mi * 60 + sec - offset)
}

/// Parses `2026-09-07T15:32:29+00:00` and `2026-09-07 15:32:29`.
fn parse_iso_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') {
        return None;
    }
    let (year, month, day) = (num(s, 0..4)?, num(s, 5..7)?, num(s, 8..10)?);
    let (h, mi, sec) = (num(s, 11..13)?, num(s, 14..16)?, num(s, 17..19)?);
    let mut offset = 0;
    if b.len() >= 25 && (b[19] == b'+' || b[19] == b'-') && b[22] == b':' {
        let o = num(s, 20..22)? * 3600 + num(s, 23..25)? * 60;
        offset = if b[19] == b'-' { -o } else { o };
    } else if b.len() != 19 && !(b.len() == 20 && b[19] == b'Z') {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86400 + h * 3600 + mi * 60 + sec - offset)
}

/// Timestamp of a log time string, zero when no known layout fits.
pub fn parse_time(s: &str) -> i64 {
    parse_nginx_time(s).or_else(|| parse_iso_time(s)).unwrap_or(0)
}

/// Returns the text up to the next space and the rest after it.
fn split_field(s: &str) -> (&str, &str) {
    match s.find(' ') {
        Some(i) => (&s[..i], s[i + 1..].trim_start_matches(' ')),
        None => (s, ""),
    }
}

/// Returns a quoted field without its quotes and the rest of the line.
fn split_quoted(s: &str) -> Option<(&str, &str)> {
    let s = s.strip_prefix('"')?;
    let end = s.find('"')?;
    Some((&s[..end], s[end + 1..].trim_start_matches(' ')))
}

/// Cache of the last parsed time string, log lines repeat it within a second.
#[derive(Default)]
pub struct TimeCache {
    text: String,
    ts: i64,
}

/// Parses one line. `None` means the line is not an access log line.
pub fn parse_line<'a>(line: &'a str, cache: &mut TimeCache) -> Option<Entry<'a>> {
    if line.len() < 20 || line.len() > MAX_LINE_LEN {
        return None;
    }
    let mut e = Entry::default();
    let (ip, rest) = split_field(line);
    e.ip = ip;
    let (_, rest) = split_field(rest);
    let (_, rest) = split_field(rest);
    let rest = rest.strip_prefix('[')?;
    let end = rest.find(']')?;
    let time_str = &rest[..end];
    if !time_str.is_empty() && time_str == cache.text {
        e.ts = cache.ts;
    } else {
        e.ts = parse_time(time_str);
        if e.ts != 0 {
            cache.text.clear();
            cache.text.push_str(time_str);
            cache.ts = e.ts;
        }
    }
    let rest = rest[end + 1..].trim_start_matches(' ');
    let (request, rest) = split_quoted(rest)?;
    let mut parts = request.split_whitespace();
    if let Some(m) = parts.next() {
        if METHODS.contains(&m) {
            e.method = m;
        }
    }
    e.path = parts.next().unwrap_or("");
    e.protocol = parts.next().unwrap_or("");
    let (status, rest) = split_field(rest);
    if let Ok(s) = status.parse::<u64>() {
        if (100..600).contains(&s) {
            e.status = s;
        }
    }
    let (size, rest) = split_field(rest);
    e.bytes_sent = size.parse::<u64>().unwrap_or(0);
    let mut rest = rest;
    if let Some((r, next)) = split_quoted(rest) {
        e.referer = r;
        rest = next;
        if let Some((ua, next)) = split_quoted(rest) {
            e.user_agent = ua;
            rest = next;
        }
    }
    let (rt, rest) = split_field(rest);
    e.request_time = parse_seconds(rt);
    let (ut, _) = split_field(rest);
    e.upstream_time = parse_seconds(ut);
    Some(e)
}

fn parse_seconds(s: &str) -> Option<f64> {
    if s.is_empty() || s == "-" {
        return None;
    }
    s.parse::<f64>().ok().filter(|v| *v >= 0.0)
}

/// Whether a parsed entry is worth indexing. Lines with binary garbage, a
/// timestamp that is missing or far in the future and malformed fields are
/// dropped, like the filter of the Go plugin.
pub fn is_valid(e: &Entry<'_>, raw: &str, now: i64) -> bool {
    if !e.ip.is_empty()
        && e.ip != "-"
        && (e.ip.contains("http")
            || e.ip.contains('/')
            || e.ip.contains("\\x")
            || e.ip.contains('%')
            || e.ip.len() > 45)
    {
        return false;
    }
    if e.ts <= 0 || e.ts > now + 86400 {
        return false;
    }
    if e.path.contains("\\x") {
        return false;
    }
    !(raw.contains("\\x16\\x03") || raw.contains("\\xFF\\xD8"))
}

/// Severity levels of the nginx error log, from the lowest.
pub const ERROR_LEVELS: [&str; 8] = ["debug", "info", "notice", "warn", "error", "crit", "alert", "emerg"];

/// One parsed error log line. Text fields borrow from the line.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ErrorEntry<'a> {
    pub ts: i64,
    pub level: &'a str,
    pub pid: u64,
    /// Connection number, zero when the line names none.
    pub connection: u64,
    pub message: &'a str,
    pub client: &'a str,
    pub server: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    pub request: &'a str,
    pub upstream: &'a str,
    pub host: &'a str,
    pub referrer: &'a str,
}

/// Parses `2026/09/07 15:32:29` in the local zone.
fn parse_error_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 19 || b[4] != b'/' || b[7] != b'/' || b[10] != b' ' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| num(s, r).and_then(|v| u32::try_from(v).ok());
    crate::localtime::from_local(num(s, 0..4)? as i32, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?)
}

/// The context nginx appends to a message, in the order it writes it. Quoted
/// values escape their own quotes as `\x22`.
const CONTEXT_KEYS: [(&str, bool); 6] = [
    (", client: ", false),
    (", server: ", false),
    (", request: ", true),
    (", upstream: ", true),
    (", host: ", true),
    (", referrer: ", true),
];

/// Parses one error log line:
/// `2026/09/07 15:32:29 [error] 12#12: *34 message, client: 1.2.3.4, ...`.
/// `None` means the line does not start like an error log entry.
pub fn parse_error_line<'a>(line: &'a str, cache: &mut TimeCache) -> Option<ErrorEntry<'a>> {
    if line.len() < 25 || line.len() > MAX_LINE_LEN {
        return None;
    }
    let time_str = line.get(..19)?;
    let ts = if time_str == cache.text {
        cache.ts
    } else {
        let ts = parse_error_time(time_str)?;
        cache.text.clear();
        cache.text.push_str(time_str);
        cache.ts = ts;
        ts
    };
    let rest = line[19..].strip_prefix(" [")?;
    let end = rest.find(']')?;
    let level = &rest[..end];
    if !ERROR_LEVELS.contains(&level) {
        return None;
    }
    let mut e = ErrorEntry { ts, level, ..Default::default() };
    let mut rest = rest[end + 1..].trim_start_matches(' ');

    // "12#34: " names the process and the thread
    if let Some(colon) = rest.find(": ") {
        if let Some((pid, tid)) = rest[..colon].split_once('#') {
            if let (Ok(pid), true) = (pid.parse::<u64>(), tid.bytes().all(|b| b.is_ascii_digit())) {
                e.pid = pid;
                rest = &rest[colon + 2..];
            }
        }
    }
    // "*56 " names the connection
    if let Some(after) = rest.strip_prefix('*') {
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after.as_bytes().get(digits) == Some(&b' ') {
            e.connection = after[..digits].parse().unwrap_or(0);
            rest = &after[digits + 1..];
        }
    }

    // The message ends where the first context key starts
    let first = CONTEXT_KEYS.iter().filter_map(|(k, _)| rest.find(k)).min().unwrap_or(rest.len());
    e.message = &rest[..first];
    let mut tail = &rest[first..];
    for (key, quoted) in CONTEXT_KEYS {
        let Some(after) = tail.strip_prefix(key) else { continue };
        let (value, next) = if quoted {
            match split_quoted(after) {
                Some((v, next)) => (v, next),
                None => break,
            }
        } else {
            let end = after.find(", ").unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        match key {
            ", client: " => e.client = value,
            ", server: " => e.server = value,
            ", request: " => e.request = value,
            ", upstream: " => e.upstream = value,
            ", host: " => e.host = value,
            _ => e.referrer = value,
        }
        tail = next.trim_start_matches(' ');
        if !tail.is_empty() && !tail.starts_with(", ") {
            break;
        }
    }
    if !e.request.is_empty() {
        let mut parts = e.request.split_whitespace();
        if let Some(m) = parts.next().filter(|m| METHODS.contains(m)) {
            e.method = m;
            e.path = parts.next().unwrap_or("");
        }
    }
    Some(e)
}

/// Whether a parsed error entry is worth indexing: its time is known and not
/// far in the future.
pub fn is_valid_error(e: &ErrorEntry<'_>, now: i64) -> bool {
    e.ts > 0 && e.ts <= now + 86400
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = r#"203.0.113.9 - - [07/Sep/2026:15:32:29 +0000] "GET /a/b?x=1 HTTP/2.0" 200 512 "https://example.com/" "Mozilla/5.0 (X11; Linux x86_64) Chrome/126.0.0.0" 0.012 0.010"#;

    #[test]
    fn parses_a_combined_line() {
        let mut c = TimeCache::default();
        let e = parse_line(LINE, &mut c).unwrap();
        assert_eq!(e.ip, "203.0.113.9");
        assert_eq!(e.method, "GET");
        assert_eq!(e.path, "/a/b?x=1");
        assert_eq!(e.protocol, "HTTP/2.0");
        assert_eq!(e.status, 200);
        assert_eq!(e.bytes_sent, 512);
        assert_eq!(e.referer, "https://example.com/");
        assert!(e.user_agent.contains("Chrome/126"));
        assert_eq!(e.request_time, Some(0.012));
        assert_eq!(e.upstream_time, Some(0.010));
        assert_eq!(e.ts, 1_788_795_149);
        assert!(is_valid(&e, LINE, 1_790_000_000));
    }

    #[test]
    fn handles_zone_offsets_and_fallback_layouts() {
        assert_eq!(parse_time("07/Sep/2026:15:32:29 +0800"), parse_time("07/Sep/2026:07:32:29 +0000"));
        assert_eq!(parse_time("2026-09-07T15:32:29+00:00"), parse_time("07/Sep/2026:15:32:29 +0000"));
        assert_eq!(parse_time("2026-09-07 15:32:29"), parse_time("07/Sep/2026:15:32:29 +0000"));
        assert_eq!(parse_time("07/Sep/2026:15:32:29"), parse_time("07/Sep/2026:15:32:29 +0000"));
        assert_eq!(parse_time("garbage"), 0);
    }

    #[test]
    fn missing_fields_stay_empty() {
        let line = r#"10.0.0.1 - - [07/Sep/2026:15:32:29 +0000] "-" 400 0 "-" "-" - -"#;
        let mut c = TimeCache::default();
        let e = parse_line(line, &mut c).unwrap();
        assert_eq!(e.method, "");
        assert_eq!(e.status, 400);
        assert_eq!(e.request_time, None);
    }

    #[test]
    fn rejects_bad_lines() {
        let mut c = TimeCache::default();
        assert!(parse_line("short", &mut c).is_none());
        let binary = r#"10.0.0.1 - - [07/Sep/2026:15:32:29 +0000] "\x16\x03\x01" 400 0 "-" "-""#;
        let e = parse_line(binary, &mut c).unwrap();
        assert!(!is_valid(&e, binary, 1_790_000_000));
        let old = r#"10.0.0.1 - - [bad] "GET / HTTP/1.1" 200 0 "-" "-""#;
        let e = parse_line(old, &mut c).unwrap();
        assert!(!is_valid(&e, old, 1_790_000_000));
    }

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        crate::localtime::from_local(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn an_error_line_with_its_context() {
        let line = r#"2026/09/07 15:32:29 [error] 1234#5678: *90 open() "/var/www/favicon.ico" failed (2: No such file or directory), client: 203.0.113.9, server: example.com, request: "GET /favicon.ico HTTP/1.1", upstream: "http://127.0.0.1:8080/favicon.ico", host: "example.com", referrer: "https://example.com/a, b""#;
        let e = parse_error_line(line, &mut TimeCache::default()).unwrap();
        assert_eq!(e.ts, local(2026, 9, 7, 15, 32, 29));
        assert_eq!((e.level, e.pid, e.connection), ("error", 1234, 90));
        assert_eq!(e.message, r#"open() "/var/www/favicon.ico" failed (2: No such file or directory)"#);
        assert_eq!((e.client, e.server), ("203.0.113.9", "example.com"));
        assert_eq!((e.method, e.path), ("GET", "/favicon.ico"));
        assert_eq!(e.upstream, "http://127.0.0.1:8080/favicon.ico");
        assert_eq!(e.host, "example.com");
        assert_eq!(e.referrer, "https://example.com/a, b");
    }

    #[test]
    fn an_error_line_without_connection_or_context() {
        let e = parse_error_line("2026/09/07 15:32:29 [notice] 1#1: signal process started", &mut TimeCache::default())
            .unwrap();
        assert_eq!((e.level, e.pid, e.connection), ("notice", 1, 0));
        assert_eq!(e.message, "signal process started");
        assert_eq!((e.client, e.request, e.method), ("", "", ""));

        let e = parse_error_line(
            "2026/09/07 15:32:29 [warn] 7#7: *3 an upstream response is buffered, client: ::1, server: _",
            &mut TimeCache::default(),
        )
        .unwrap();
        assert_eq!((e.client, e.server), ("::1", "_"));
    }

    #[test]
    fn lines_that_are_not_error_entries() {
        let mut cache = TimeCache::default();
        assert!(parse_error_line("    at continuation of a previous message", &mut cache).is_none());
        assert!(parse_error_line("2026/09/07 15:32:29 [verbose] 1#1: x", &mut cache).is_none());
        assert!(parse_error_line("2026/13/07 15:32:29 [error] 1#1: x", &mut cache).is_none());
        let access = r#"1.2.3.4 - - [07/Sep/2026:15:32:29 +0000] "GET / HTTP/1.1" 200 5 "-" "-""#;
        assert!(parse_error_line(access, &mut cache).is_none());
    }

    #[test]
    fn error_entries_in_the_future_are_dropped() {
        let e = parse_error_line("2026/09/07 15:32:29 [error] 1#1: x", &mut TimeCache::default()).unwrap();
        assert!(is_valid_error(&e, e.ts));
        assert!(!is_valid_error(&e, e.ts - 2 * 86400));
    }
}
