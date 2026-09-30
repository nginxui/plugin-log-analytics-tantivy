//! Log aware tokenizer for the raw line, path, referer and user agent fields.
//!
//! Text is first unescaped (`%HH` and nginx `\xHH` runs become UTF-8 text), then split
//! into alphanumeric atoms. Every unit takes one position; extra tokens share it.
//! Mode::Full (raw line field) adds:
//! - prefixes of dotted numbers (`126.0.0.0` gives `126`, `126.0`, `126.0.0`, `126.0.0.0`)
//!   instead of one atom per group, so IPs, versions and `10_15_7` stay searchable by prefix
//! - canonical IPv6 addresses as one token
//! - CJK bigrams next to the single characters
//!
//! Both modes emit emoji as tokens and a `../` token for path traversal.
//!
//! Mode::Atoms (path, referer, user agent) emits only atoms, CJK characters, emoji and `../`,
//! because those fields are searched with phrase queries. In query mode a dotted number
//! or a CJK run yields only its widest token(s).

use std::net::Ipv6Addr;

use tantivy::tokenizer::{LowerCaser, RemoveLongFilter, TextAnalyzer, Token, TokenStream, Tokenizer};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Full,
    Atoms,
}

#[derive(Clone)]
pub struct LogTokenizer {
    mode: Mode,
    query: bool,
    tokens: Vec<Token>,
    len: usize,
}

pub struct LogStream<'a> {
    tokens: &'a mut [Token],
    next: usize,
}

impl LogTokenizer {
    pub fn new(mode: Mode, query: bool) -> Self {
        Self { mode, query, tokens: Vec::new(), len: 0 }
    }

    fn push(&mut self, text: &str, from: usize, to: usize, position: usize) {
        if self.len == self.tokens.len() {
            self.tokens.push(Token::default());
        }
        let t = &mut self.tokens[self.len];
        t.text.clear();
        t.text.push_str(text);
        t.offset_from = from;
        t.offset_to = to;
        t.position = position;
        t.position_length = 1;
        self.len += 1;
    }
}

impl Tokenizer for LogTokenizer {
    type TokenStream<'a> = LogStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> LogStream<'a> {
        self.len = 0;
        match unescape(text) {
            Some(s) => self.scan(&s),
            None => self.scan(text),
        }
        LogStream { tokens: &mut self.tokens[..self.len], next: 0 }
    }
}

impl TokenStream for LogStream<'_> {
    fn advance(&mut self) -> bool {
        if self.next < self.tokens.len() {
            self.next += 1;
            true
        } else {
            false
        }
    }

    fn token(&self) -> &Token {
        &self.tokens[self.next - 1]
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.next - 1]
    }
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// Byte value of a `%HH` or `\xHH` escape at the start of `b`, with its length.
fn escape_at(b: &[u8]) -> Option<(u8, usize)> {
    match b {
        [b'%', h, l, ..] => Some((hex(*h)? << 4 | hex(*l)?, 3)),
        [b'\\', b'x', h, l, ..] => Some((hex(*h)? << 4 | hex(*l)?, 4)),
        _ => None,
    }
}

/// Decodes escape runs into text. Returns None when the input has no escapes.
fn unescape(s: &str) -> Option<String> {
    if !s.contains('%') && !s.contains("\\x") {
        return None;
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut plain) = (0, 0);
    while i < b.len() {
        if escape_at(&b[i..]).is_none() {
            i += 1;
            continue;
        }
        out.push_str(&s[plain..i]);
        let mut bytes = Vec::new();
        while let Some((v, n)) = escape_at(&b[i..]) {
            bytes.push(v);
            i += n;
        }
        out.push_str(&String::from_utf8_lossy(&bytes));
        plain = i;
    }
    out.push_str(&s[plain..]);
    Some(out)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F)
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B50 | 0x2B55)
}

/// Letters and digits that belong to an atom (CJK and emoji are handled on their own).
fn is_atom_char(c: char) -> bool {
    c.is_alphanumeric() && !is_cjk(c) && !is_emoji(c)
}

impl LogTokenizer {
    /// Splits a text into tokens. Pure ASCII text takes a byte based path that
    /// gives the same tokens as the general one.
    fn scan(&mut self, s: &str) {
        if s.is_ascii() {
            self.scan_ascii(s);
        } else {
            self.scan_general(s);
        }
    }

    /// The ASCII path: no character decoding, no CJK or emoji.
    fn scan_ascii(&mut self, s: &str) {
        let full = self.mode == Mode::Full;
        let b = s.as_bytes();
        let mut i = 0;
        let mut pos = 0;
        while i < b.len() {
            let c = b[i];
            if full
                && (c.is_ascii_hexdigit() || c == b':')
                && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
            {
                if let Some((end, canon)) = ipv6_at(s, i) {
                    self.push(&canon, i, end, pos);
                    pos += 1;
                    i = end;
                    continue;
                }
            }
            if c.is_ascii_alphanumeric() {
                let mut end = i + 1;
                while end < b.len() && b[end].is_ascii_alphanumeric() {
                    end += 1;
                }
                if full && c.is_ascii_digit() {
                    if let Some(groups) = number_run(s, i) {
                        self.push_number(s, i, &groups, pos);
                        pos += 1;
                        i = *groups.last().unwrap();
                        continue;
                    }
                }
                self.push(&s[i..end], i, end, pos);
                pos += 1;
                i = end;
            } else if c == b'.' && (b[i..].starts_with(b"../") || b[i..].starts_with(b"..\\")) {
                self.push("../", i, i + 3, pos);
                pos += 1;
                i += 3;
            } else {
                i += 1;
            }
        }
    }

    fn scan_general(&mut self, s: &str) {
        let full = self.mode == Mode::Full;
        let b = s.as_bytes();
        let mut i = 0;
        let mut pos = 0;
        while i < s.len() {
            let c = s[i..].chars().next().unwrap();
            if full && (c.is_ascii_hexdigit() || c == ':') && word_start(s, i) {
                if let Some((end, canon)) = ipv6_at(s, i) {
                    self.push(&canon, i, end, pos);
                    pos += 1;
                    i = end;
                    continue;
                }
            }
            if is_atom_char(c) {
                let end = atom_end(s, i);
                if full && b[i].is_ascii_digit() {
                    if let Some(groups) = number_run(s, i) {
                        self.push_number(s, i, &groups, pos);
                        pos += 1;
                        i = *groups.last().unwrap();
                        continue;
                    }
                }
                self.push(&s[i..end], i, end, pos);
                pos += 1;
                i = end;
            } else if is_cjk(c) {
                let start = i;
                let mut chars = Vec::new();
                while i < s.len() {
                    let c = s[i..].chars().next().unwrap();
                    if !is_cjk(c) {
                        break;
                    }
                    chars.push((i, i + c.len_utf8()));
                    i += c.len_utf8();
                }
                self.push_cjk(s, &chars, pos);
                pos += chars.len();
                debug_assert!(i > start);
            } else if is_emoji(c) {
                self.push(&s[i..i + c.len_utf8()], i, i + c.len_utf8(), pos);
                pos += 1;
                i += c.len_utf8();
            } else if s[i..].starts_with("../") || s[i..].starts_with("..\\") {
                self.push("../", i, i + 3, pos);
                pos += 1;
                i += 3;
            } else {
                i += c.len_utf8();
            }
        }
    }

    /// Dotted number: all prefixes at one position, or only the whole number in query mode.
    fn push_number(&mut self, s: &str, start: usize, groups: &[usize], pos: usize) {
        let from = if self.query { groups.len() - 1 } else { 0 };
        for &end in &groups[from..] {
            self.push(&s[start..end], start, end, pos);
        }
    }

    /// CJK run: single characters, plus bigrams (only bigrams for a longer run in query mode).
    fn push_cjk(&mut self, s: &str, chars: &[(usize, usize)], pos: usize) {
        let full = self.mode == Mode::Full;
        for (k, &(from, to)) in chars.iter().enumerate() {
            let skip_single = self.query && full && chars.len() > 1;
            if !skip_single {
                self.push(&s[from..to], from, to, pos + k);
            }
            if full {
                if let Some(&(_, next_to)) = chars.get(k + 1) {
                    self.push(&s[from..next_to], from, next_to, pos + k);
                }
            }
        }
    }
}

/// True when the byte before `i` does not continue a word.
fn word_start(s: &str, i: usize) -> bool {
    s[..i].chars().next_back().is_none_or(|p| !(p.is_alphanumeric() || p == '_'))
}

fn atom_end(s: &str, from: usize) -> usize {
    let mut end = from;
    for c in s[from..].chars() {
        if !is_atom_char(c) {
            break;
        }
        end += c.len_utf8();
    }
    end
}

/// Ends of the groups of a dotted (or `_` joined) digit run starting at `from`, when it has
/// at least two groups and does not run into letters.
fn number_run(s: &str, from: usize) -> Option<Vec<usize>> {
    let b = s.as_bytes();
    let digits = |mut i: usize| {
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        i
    };
    let mut end = digits(from);
    let mut groups = vec![end];
    while end + 1 < b.len() && (b[end] == b'.' || b[end] == b'_') && b[end + 1].is_ascii_digit() {
        end = digits(end + 1);
        groups.push(end);
    }
    let glued = s[end..].chars().next().is_some_and(is_atom_char);
    (groups.len() > 1 && !glued).then_some(groups)
}

/// IPv6 address at `from` as its canonical text and the end offset.
fn ipv6_at(s: &str, from: usize) -> Option<(usize, String)> {
    let b = s.as_bytes();
    let mut end = from;
    let mut colons = 0;
    while end < b.len() && (b[end].is_ascii_hexdigit() || b[end] == b':' || b[end] == b'.') {
        colons += (b[end] == b':') as usize;
        end += 1;
    }
    if colons < 2 {
        return None;
    }
    loop {
        if let Ok(ip) = s[from..end].parse::<Ipv6Addr>() {
            return Some((end, ip.to_string()));
        }
        match b.get(end - 1) {
            Some(b':' | b'.') if end - 1 > from => end -= 1,
            _ => return None,
        }
    }
}

/// Name of the analyzer of the raw line field.
pub const TEXT_NAME: &str = "log_text";
/// Name of the analyzer of the path, referer and user agent fields.
pub const PHRASE_NAME: &str = "log_phrase";

fn analyzer_for(mode: Mode, query: bool) -> TextAnalyzer {
    TextAnalyzer::builder(LogTokenizer::new(mode, query))
        .filter(RemoveLongFilter::limit(255))
        .filter(LowerCaser)
        .build()
}

/// Index side analyzer: the raw line (`phrase` false) or a phrase field.
pub fn analyzer(phrase: bool) -> TextAnalyzer {
    analyzer_for(if phrase { Mode::Atoms } else { Mode::Full }, false)
}

/// Registers both analyzers on an index. This is needed after create and
/// after open, since tantivy does not store analyzers.
pub fn register(index: &tantivy::Index) {
    index.tokenizers().register(TEXT_NAME, analyzer(false));
    index.tokenizers().register(PHRASE_NAME, analyzer(true));
}

fn run(mut a: TextAnalyzer, text: &str) -> Vec<(usize, String)> {
    let mut stream = a.token_stream(text);
    let mut out = Vec::new();
    while stream.advance() {
        let t = stream.token();
        out.push((t.position, t.text.clone()));
    }
    out
}

/// Tokens the indexer writes, as (position, term).
pub fn index_tokens(text: &str, phrase: bool) -> Vec<(usize, String)> {
    run(analyzer(phrase), text)
}

/// Tokens of a search box (`phrase` false) or a filter as (position, term).
/// A dotted number or a run of CJK characters keeps only its widest tokens.
pub fn query_tokens(text: &str, phrase: bool) -> Vec<(usize, String)> {
    run(analyzer_for(if phrase { Mode::Atoms } else { Mode::Full }, true), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(text: &str, phrase: bool) -> Vec<String> {
        index_tokens(text, phrase).into_iter().map(|(_, t)| t).collect()
    }

    fn query(text: &str, phrase: bool) -> Vec<String> {
        query_tokens(text, phrase).into_iter().map(|(_, t)| t).collect()
    }

    #[test]
    fn dotted_numbers_emit_every_prefix_at_one_position() {
        let t = index_tokens("126.0.0.0", false);
        assert_eq!(t.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>(), ["126", "126.0", "126.0.0", "126.0.0.0"]);
        assert!(t.iter().all(|(p, _)| *p == 0));
        assert_eq!(query("126.0.0.0", false), ["126.0.0.0"]);
        assert_eq!(query("192.168", false), ["192.168"]);
        assert!(terms("from 192.168.1.10 now", false).contains(&"192.168".to_owned()));
    }

    #[test]
    fn escapes_are_decoded_so_both_forms_match() {
        assert_eq!(terms("union%20select", false), ["union", "select"]);
        assert_eq!(terms("a\\x20b", false), ["a", "b"]);
        assert_eq!(terms("%E4%B8%AD%E6%96%87", false), terms("中文", false));
        assert!(terms("%2e%2e%2fetc", false).contains(&"../".to_owned()));
    }

    #[test]
    fn no_stop_words_and_lowercase() {
        assert_eq!(terms("/How-To/About", true), ["how", "to", "about"]);
        assert_eq!(terms("The AND of", false), ["the", "and", "of"]);
    }

    #[test]
    fn ipv6_is_one_canonical_token() {
        assert_eq!(terms("2001:0DB8:0000:0000:0000:0000:0000:0001", false), ["2001:db8::1"]);
        assert_eq!(query("2001:db8::1", false), ["2001:db8::1"]);
    }

    #[test]
    fn cjk_gets_characters_and_bigrams_with_a_bigram_only_query() {
        let idx = terms("中文", false);
        assert_eq!(idx, ["中", "中文", "文"]);
        assert_eq!(query("中文", false), ["中文"]);
        assert_eq!(query("中", false), ["中"]);
        // The phrase variant keeps single characters only
        assert_eq!(terms("中文", true), ["中", "文"]);
    }

    #[test]
    fn emoji_and_traversal_are_tokens() {
        assert!(terms("hello 😀", false).contains(&"😀".to_owned()));
        assert!(terms("/a/../b", true).contains(&"../".to_owned()));
    }

    /// Tokens of both scan paths as (text, from, to, position).
    type Seen = Vec<(String, usize, usize, usize)>;

    fn both_paths(mode: Mode, query: bool, text: &str) -> (Seen, Seen) {
        let mut t = LogTokenizer::new(mode, query);
        let grab = |t: &LogTokenizer| {
            t.tokens[..t.len]
                .iter()
                .map(|k| (k.text.clone(), k.offset_from, k.offset_to, k.position))
                .collect::<Vec<_>>()
        };
        t.len = 0;
        t.scan_ascii(text);
        let fast = grab(&t);
        t.len = 0;
        t.scan_general(text);
        (fast, grab(&t))
    }

    /// Small xorshift generator, so the test needs no extra crate.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    #[test]
    fn ascii_path_matches_the_general_path_on_random_text() {
        // Characters that steer the scanner: digits, dots, colons, hex letters, escapes, separators
        const PIECES: &[&str] = &[
            "0",
            "1",
            "9",
            "12",
            "255",
            ".",
            ".",
            "..",
            "../",
            "..\\",
            "_",
            ":",
            "::",
            "a",
            "f",
            "e",
            "x",
            "z",
            "Q",
            "-",
            "/",
            " ",
            "%",
            "%2",
            "%20",
            "%2e",
            "\\x",
            "\\x41",
            "\"",
            "[",
            "]",
            "?",
            "=",
            "&",
            "2001:db8::1",
            "::1",
            "192.168.1.10",
            "126.0.0.0",
            "10_15_7",
            "abc",
            "DEAD",
            "beef",
            "1.2.",
            "1.",
            ".1",
            "a1",
            "1a",
        ];
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..300_000 {
            let n = 1 + (rng.next() % 12) as usize;
            let mut text = String::new();
            for _ in 0..n {
                text.push_str(PIECES[(rng.next() % PIECES.len() as u64) as usize]);
            }
            // Mix in plain random ASCII bytes
            if rng.next().is_multiple_of(4) {
                for _ in 0..(rng.next() % 6) {
                    text.push((0x20 + (rng.next() % 95) as u8) as char);
                }
            }
            for (mode, query) in [(Mode::Full, false), (Mode::Full, true), (Mode::Atoms, false), (Mode::Atoms, true)] {
                let (fast, slow) = both_paths(mode, query, &text);
                assert_eq!(fast, slow, "input {text:?}");
            }
        }
    }

    #[test]
    fn ascii_path_matches_the_general_path_on_log_lines() {
        let lines = [
            r#"39.61.3.207 - - [07/Sep/2026:15:32:29 +0000] "POST /api/v1/health HTTP/1.1" 200 2089 "-" "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36" 0.127 0.094"#,
            r#"2001:db8::1 - - [07/Sep/2026:15:32:30 +0000] "GET /a/../../etc/passwd?x=1%20union%20select HTTP/2.0" 404 0 "https://example.com/blog/?q=a%2Fb" "curl/8.4" 0.001 -"#,
            r#"::ffff:10.0.0.1 - - [07/Sep/2026:15:32:31 +0000] "GET /\x22x\x22 HTTP/1.1" 400 12 "-" "-" 0.000 -"#,
        ];
        // The real dataset adds volume when it is there
        let mut all: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
        if let Ok(text) = std::fs::read_to_string("/Volumes/Working/Git/.tmp-635799/perf/data/access.log.1") {
            all.extend(text.lines().step_by(7).take(20_000).map(str::to_owned));
        }
        for line in all.iter().filter(|l| l.is_ascii()) {
            for (mode, query) in [(Mode::Full, false), (Mode::Atoms, false), (Mode::Full, true)] {
                let (fast, slow) = both_paths(mode, query, line);
                assert_eq!(fast, slow, "input {line:?}");
            }
        }
    }

    /// Prints the time of both paths over the dataset lines, run on request.
    #[test]
    #[ignore = "measurement, run with --release -- --ignored --nocapture"]
    fn ascii_path_speed() {
        let text = std::fs::read_to_string("/Volumes/Working/Git/.tmp-635799/perf/data/access.log").unwrap();
        let lines: Vec<&str> = text.lines().take(300_000).collect();
        for mode in [Mode::Full, Mode::Atoms] {
            let mut t = LogTokenizer::new(mode, false);
            let started = std::time::Instant::now();
            let mut n = 0;
            for l in &lines {
                t.len = 0;
                t.scan_general(l);
                n += t.len;
            }
            let slow = started.elapsed();
            let started = std::time::Instant::now();
            for l in &lines {
                t.len = 0;
                t.scan_ascii(l);
                n += t.len;
            }
            eprintln!("{} lines, {n} tokens: general {slow:?}, ascii {:?}", lines.len(), started.elapsed());
        }
    }

    #[test]
    fn phrase_fields_keep_atoms_only() {
        assert_eq!(terms("Chrome/126.0.0.0", true), ["chrome", "126", "0", "0", "0"]);
        assert_eq!(terms("wp-login.php", true), ["wp", "login", "php"]);
    }
}
