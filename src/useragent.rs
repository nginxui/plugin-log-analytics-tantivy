//! User agent detection. The ordered rules and their version patterns are the
//! ones of the Go plugin, so both plugins name browsers, systems and devices
//! the same way.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use regex::Regex;

/// Browser rules: first match wins. The optional pattern reads the version.
const BROWSERS: &[(&str, &str, Option<&str>)] = &[
    (
        "Bot",
        r#"(?i)bot|crawler|spider|crawl|slurp|sohu-search|lycos|robozilla|googlebot|bingbot|facebookexternalhit|twitterbot|whatsapp|telegrambot|applebot|linkedinbot|pinterest|yandexbot|baiduspider|360spider|sogou|bytedance|tiktok"#,
        None,
    ),
    ("WeChat", r#"(?i)micromessenger"#, Some(r#"(?i)micromessenger/(\d+\.\d+)"#)),
    ("QQ", r#"(?i)qq/(\d+\.\d+)"#, Some(r#"(?i)qq/(\d+\.\d+)"#)),
    ("DingTalk", r#"(?i)dingtalk"#, Some(r#"(?i)dingtalk/(\d+\.\d+)"#)),
    ("Alipay", r#"(?i)alipayclient"#, Some(r#"(?i)alipayclient/(\d+\.\d+)"#)),
    ("TikTok", r#"(?i)musically_"#, Some(r#"(?i)musically_(\d+\.\d+)"#)),
    ("360 Browser", r#"(?i)360se|qihoobrowser"#, Some(r#"(?i)360se/(\d+\.\d+)|qihoobrowser/(\d+\.\d+)"#)),
    ("QQ Browser", r#"(?i)qqbrowser"#, Some(r#"(?i)qqbrowser/(\d+\.\d+)"#)),
    ("UC Browser", r#"(?i)ucbrowser|uc browser"#, Some(r#"(?i)ucbrowser/(\d+\.\d+)"#)),
    ("Sogou Explorer", r#"(?i)se |metasr"#, Some(r#"(?i)se (\d+\.\d+)|metasr (\d+\.\d+)"#)),
    ("Baidu Browser", r#"(?i)baidubrowser|bidubrowser"#, Some(r#"(?i)baidubrowser/(\d+\.\d+)|bidubrowser/(\d+\.\d+)"#)),
    ("Maxthon", r#"(?i)maxthon"#, Some(r#"(?i)maxthon/(\d+\.\d+)"#)),
    ("Samsung Browser", r#"(?i)samsungbrowser"#, Some(r#"(?i)samsungbrowser/(\d+\.\d+)"#)),
    ("Huawei Browser", r#"(?i)huaweibrowser"#, Some(r#"(?i)huaweibrowser/(\d+\.\d+)"#)),
    ("Xiaomi Browser", r#"(?i)mibrowser"#, Some(r#"(?i)mibrowser/(\d+\.\d+)"#)),
    ("Oppo Browser", r#"(?i)oppobrowser"#, Some(r#"(?i)oppobrowser/(\d+\.\d+)"#)),
    ("Vivo Browser", r#"(?i)vivobrowser"#, Some(r#"(?i)vivobrowser/(\d+\.\d+)"#)),
    ("Yandex", r#"(?i)yabrowser"#, Some(r#"(?i)yabrowser/(\d+\.\d+)"#)),
    ("Brave", r#"(?i)brave"#, Some(r#"(?i)brave/(\d+\.\d+)"#)),
    ("Vivaldi", r#"(?i)vivaldi"#, Some(r#"(?i)vivaldi/(\d+\.\d+)"#)),
    ("Edge", r#"(?i)edg/|edge/"#, Some(r#"(?i)edg?[e]?/(\d+\.\d+)"#)),
    ("Internet Explorer", r#"(?i)msie |trident.*rv:"#, Some(r#"(?i)msie (\d+\.\d+)|rv:(\d+\.\d+)"#)),
    ("Opera", r#"(?i)opr/|opera/"#, Some(r#"(?i)opr/(\d+\.\d+)|version/(\d+\.\d+)"#)),
    ("Chrome", r#"(?i)chrome/"#, Some(r#"(?i)chrome/(\d+\.\d+)"#)),
    ("Firefox", r#"(?i)firefox/"#, Some(r#"(?i)firefox/(\d+\.\d+)"#)),
    ("Safari", r#"(?i)safari/"#, Some(r#"(?i)version/(\d+\.\d+)"#)),
    ("NetFront", r#"(?i)netfront"#, Some(r#"(?i)netfront/(\d+\.\d+)"#)),
    ("Konqueror", r#"(?i)konqueror"#, Some(r#"(?i)konqueror/(\d+\.\d+)"#)),
];

/// Operating system rules: first match wins.
const SYSTEMS: &[(&str, &str, Option<&str>)] = &[
    ("iOS", r#"(?i)iPhone OS|OS (\d+_\d+)|iPad; OS|iPod.*OS|iPhone.*OS"#, Some(r#"(?i)OS (\d+[_\d]*)"#)),
    ("Android", r#"(?i)android"#, Some(r#"(?i)android (\d+\.?\d*\.?\d*)"#)),
    ("Windows", r#"(?i)windows"#, Some(r#"(?i)windows nt (\d+\.?\d*)"#)),
    ("macOS", r#"(?i)mac os x|macintosh|intel mac"#, Some(r#"(?i)mac os x (\d+[_\d]*)"#)),
    ("Ubuntu", r#"(?i)ubuntu"#, Some(r#"(?i)ubuntu[\/\s]*(\d+\.?\d*\.?\d*)"#)),
    ("CentOS", r#"(?i)centos"#, Some(r#"(?i)centos[\/\s]*(\d+\.?\d*\.?\d*)"#)),
    ("Red Hat", r#"(?i)red.*hat|rhel"#, Some(r#"(?i)red.*hat[\/\s]*(\d+\.?\d*\.?\d*)|rhel[\/\s]*(\d+\.?\d*\.?\d*)"#)),
    ("Debian", r#"(?i)debian"#, Some(r#"(?i)debian[\/\s]*(\d+\.?\d*\.?\d*)"#)),
    ("Fedora", r#"(?i)fedora"#, Some(r#"(?i)fedora[\/\s]*(\d+\.?\d*\.?\d*)"#)),
    ("SUSE", r#"(?i)suse|opensuse"#, Some(r#"(?i)suse[\/\s]*(\d+\.?\d*\.?\d*)|opensuse[\/\s]*(\d+\.?\d*\.?\d*)"#)),
    ("FreeBSD", r#"(?i)freebsd"#, Some(r#"(?i)freebsd (\d+\.?\d*\.?\d*)"#)),
    ("OpenBSD", r#"(?i)openbsd"#, Some(r#"(?i)openbsd (\d+\.?\d*\.?\d*)"#)),
    ("NetBSD", r#"(?i)netbsd"#, Some(r#"(?i)netbsd (\d+\.?\d*\.?\d*)"#)),
    ("Linux", r#"(?i)linux|x11"#, None),
    ("Chrome OS", r#"(?i)cros"#, Some(r#"(?i)cros (\d+\.?\d*\.?\d*)"#)),
    ("Windows Phone", r#"(?i)windows phone"#, Some(r#"(?i)windows phone (\d+\.?\d*\.?\d*)"#)),
    (
        "BlackBerry",
        r#"(?i)blackberry|bb10"#,
        Some(r#"(?i)blackberry[\/\s]*(\d+\.?\d*\.?\d*)|bb10[\/\s]*(\d+\.?\d*\.?\d*)"#),
    ),
    ("Symbian", r#"(?i)symbian|s60"#, Some(r#"(?i)symbian[\/\s]*(\d+\.?\d*\.?\d*)|s60[\/\s]*(\d+\.?\d*\.?\d*)"#)),
];

/// Device rules: first match wins, `Desktop` is the fallback.
const DEVICES: &[(&str, &str)] = &[
    (
        "Bot",
        r#"(?i)bot|crawler|spider|crawl|slurp|sohu-search|lycos|robozilla|googlebot|bingbot|facebookexternalhit|twitterbot|whatsapp|telegrambot|applebot|linkedinbot|pinterest|yandexbot|baiduspider|360spider|sogou|bytedance|scraper"#,
    ),
    ("iPhone", r#"(?i)iphone"#),
    ("iPad", r#"(?i)ipad"#),
    ("iPod", r#"(?i)ipod"#),
    ("Apple Watch", r#"(?i)watch.*os"#),
    ("Apple TV", r#"(?i)apple.*tv"#),
    ("PlayStation", r#"(?i)playstation|ps[345]|psvita"#),
    ("Xbox", r#"(?i)xbox"#),
    ("Nintendo", r#"(?i)nintendo|wii|3ds|switch"#),
    ("Smart TV", r#"(?i)smart.*tv|smarttv|hbbtv|netcast|roku|webos|tizen|android.*tv"#),
    ("Chromecast", r#"(?i)chromecast"#),
    (
        "Tablet",
        r#"(?i)tablet|ipad|kindle|nook|playbook|touchpad|xoom|sch-i800|gt-p1000|sgh-t849|shw-m180s|a1_07|bntv250a|mid7015|mid7012"#,
    ),
    (
        "Mobile",
        r#"(?i)mobile|phone|iphone|android.*mobile|blackberry|bb10|windows phone|iemobile|palm|webos|symbian|maemo|fennec|minimo|pda|pocket|psp|smartphone|mobileexplorer|htc|samsung|lg|motorola|sony|nokia|huawei|xiaomi|oppo|vivo|oneplus"#,
    ),
    ("Wearable", r#"(?i)watch|wearable|fitbit|gear"#),
    ("IoT Device", r#"(?i)alexa|echo|iot|raspberry|arduino"#),
    ("E-Reader", r#"(?i)kindle|nook|kobo|pocketbook"#),
];

/// Browser, system and device of one user agent. Empty strings mean unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UaInfo {
    pub browser: &'static str,
    pub os: &'static str,
    pub device: &'static str,
}

/// [`UaInfo`] with the versions, for the entries shown to a person.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UaDetail {
    pub browser: String,
    pub browser_version: String,
    pub os: String,
    pub os_version: String,
    pub device: String,
}

struct Rule {
    name: &'static str,
    pattern: Regex,
    version: Option<Regex>,
}

/// The compiled rules, shared by every thread.
pub struct UaParser {
    browsers: Vec<Rule>,
    systems: Vec<Rule>,
    devices: Vec<(&'static str, Regex)>,
}

fn compile(list: &[(&'static str, &'static str, Option<&'static str>)]) -> Vec<Rule> {
    list.iter()
        .map(|(name, pattern, version)| Rule {
            name,
            pattern: Regex::new(pattern).expect("user agent pattern"),
            version: version.map(|v| Regex::new(v).expect("user agent version pattern")),
        })
        .collect()
}

/// The shared parser, compiled on first use.
pub fn parser() -> &'static Arc<UaParser> {
    static PARSER: OnceLock<Arc<UaParser>> = OnceLock::new();
    PARSER.get_or_init(|| Arc::new(UaParser::new()))
}

impl Default for UaParser {
    fn default() -> Self {
        Self::new()
    }
}

impl UaParser {
    pub fn new() -> Self {
        Self {
            browsers: compile(BROWSERS),
            systems: compile(SYSTEMS),
            devices: DEVICES.iter().map(|(n, p)| (*n, Regex::new(p).expect("device pattern"))).collect(),
        }
    }

    fn device_of(&self, ua: &str) -> &'static str {
        let mut device = self.devices.iter().find(|(_, re)| re.is_match(ua)).map_or("Desktop", |(n, _)| *n);
        let lower = ua.to_ascii_lowercase();
        let android_only = lower.contains("android") && !lower.contains("mobile");
        if device == "Mobile" && (lower.contains("ipad") || lower.contains("tablet") || android_only) {
            device = "Tablet";
        }
        if device == "Desktop" && android_only {
            device = "Tablet";
        }
        device
    }

    /// Names only, which is what the index stores.
    pub fn detect(&self, ua: &str) -> UaInfo {
        let first = |rules: &[Rule]| rules.iter().find(|r| r.pattern.is_match(ua)).map_or("", |r| r.name);
        UaInfo {
            browser: fix_browser_name(first(&self.browsers), ua),
            os: first(&self.systems),
            device: self.device_of(ua),
        }
    }

    /// Names and versions.
    pub fn detail(&self, ua: &str) -> UaDetail {
        if ua.is_empty() || ua == "-" {
            return UaDetail::default();
        }
        let version_of = |rule: &Rule| -> String {
            rule.version
                .as_ref()
                .and_then(|re| re.captures(ua))
                .and_then(|c| c.get(1))
                .map_or(String::new(), |m| m.as_str().to_owned())
        };
        let mut out = UaDetail::default();
        if let Some(rule) = self.browsers.iter().find(|r| r.pattern.is_match(ua)) {
            out.browser = fix_browser_name(rule.name, ua).to_owned();
            out.browser_version = version_of(rule);
            if out.browser_version.is_empty() {
                out.browser_version = extra_version(rule.name, ua);
            }
        }
        if let Some(rule) = self.systems.iter().find(|r| r.pattern.is_match(ua)) {
            out.os = rule.name.to_owned();
            out.os_version = clean_version(&version_of(rule).replace('_', "."));
        }
        out.device = self.device_of(ua).to_owned();
        out
    }
}

/// Distinguishes the Chrome based browsers that report themselves as Chrome.
fn fix_browser_name(browser: &'static str, ua: &str) -> &'static str {
    if browser == "Chrome" {
        let lower = ua.to_ascii_lowercase();
        if lower.contains("edg/") {
            return "Edge";
        }
        if lower.contains("opr/") {
            return "Opera";
        }
        if lower.contains("samsungbrowser") {
            return "Samsung Browser";
        }
    }
    browser
}

/// Three part versions of the apps whose main pattern reads two parts.
fn extra_version(browser: &str, ua: &str) -> String {
    static RULES: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        [
            ("WeChat", r"(?i)micromessenger/(\d+\.\d+\.\d+)"),
            ("QQ", r"(?i)qq/(\d+\.\d+\.\d+)"),
            ("Alipay", r"(?i)alipayclient/(\d+\.\d+\.\d+)"),
        ]
        .into_iter()
        .map(|(n, p)| (n, Regex::new(p).expect("version pattern")))
        .collect()
    });
    rules
        .iter()
        .find(|(n, _)| *n == browser)
        .and_then(|(_, re)| re.captures(ua))
        .and_then(|c| c.get(1))
        .map_or(String::new(), |m| m.as_str().to_owned())
}

fn clean_version(version: &str) -> String {
    let mut v = version.replace('_', ".");
    while let Some(stripped) = v.strip_suffix(".0") {
        v = stripped.to_owned();
    }
    v
}

/// Per thread cache in front of the rules. Logs repeat a few user agents.
pub struct UaCache {
    parser: Arc<UaParser>,
    cache: HashMap<String, UaInfo>,
}

const CACHE_LIMIT: usize = 10_000;

impl UaCache {
    pub fn new(parser: Arc<UaParser>) -> Self {
        Self { parser, cache: HashMap::new() }
    }

    /// Names of a user agent, empty for a missing one or a dash.
    pub fn info(&mut self, ua: &str) -> UaInfo {
        if ua.is_empty() || ua == "-" {
            return UaInfo::default();
        }
        if let Some(info) = self.cache.get(ua) {
            return info.clone();
        }
        let info = self.parser.detect(ua);
        if self.cache.len() >= CACHE_LIMIT {
            self.cache.clear();
        }
        self.cache.insert(ua.to_owned(), info.clone());
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
    const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1";

    #[test]
    fn detects_names_and_versions() {
        let p = UaParser::new();
        let d = p.detail(CHROME);
        assert_eq!(d.browser, "Chrome");
        assert_eq!(d.browser_version, "126.0");
        assert_eq!(d.os, "Windows");
        assert_eq!(d.os_version, "10");
        assert_eq!(d.device, "Desktop");

        let d = p.detail(IPHONE);
        assert_eq!(d.browser, "Safari");
        assert_eq!(d.browser_version, "17.5");
        assert_eq!(d.os, "iOS");
        assert_eq!(d.os_version, "17.5");
        assert_eq!(d.device, "iPhone");
    }

    #[test]
    fn empty_and_dash_are_unknown() {
        let mut c = UaCache::new(parser().clone());
        assert_eq!(c.info(""), UaInfo::default());
        assert_eq!(c.info("-"), UaInfo::default());
        assert_eq!(c.info("Googlebot/2.1").browser, "Bot");
        assert_eq!(c.info("Googlebot/2.1").device, "Bot");
    }

    #[test]
    fn android_without_mobile_is_a_tablet() {
        let p = UaParser::new();
        let ua = "Mozilla/5.0 (Linux; Android 13; SM-X700) AppleWebKit/537.36 Chrome/120.0 Safari/537.36";
        assert_eq!(p.detect(ua).device, "Tablet");
    }
}
