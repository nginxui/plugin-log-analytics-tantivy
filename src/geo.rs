//! IP location: the country from the embedded database and the province, city
//! and custom fields from the GeoLite2 City database once it is installed.

use std::collections::HashMap;
use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use maxminddb::{Mmap, Reader};
use serde::Deserialize;

/// The embedded GeoLite2 country database, xz compressed.
const COUNTRY_XZ: &[u8] = include_bytes!("../assets/GeoLite2-Country.mmdb.xz");

/// Where a located address belongs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeoLocation {
    pub region_code: String,
    pub province: String,
    pub city: String,
    pub c1: String,
    pub c2: String,
    pub c3: String,
    pub c4: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Names {
    en: Option<String>,
    #[serde(rename = "zh-CN")]
    zh_cn: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Place {
    names: Names,
    name: Option<String>,
    name_zh: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CountryPlace {
    iso_code: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CityRecord {
    country: CountryPlace,
    subdivisions: Vec<Place>,
    province: Place,
    city: Place,
    c1: Option<String>,
    c2: Option<String>,
    c3: Option<String>,
    c4: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CountryRecord {
    country: CountryPlace,
}

fn first_non_empty<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> String {
    values.into_iter().flatten().map(str::trim).find(|v| !v.is_empty()).unwrap_or_default().to_owned()
}

fn trimmed(v: &Option<String>) -> String {
    v.as_deref().map(str::trim).unwrap_or_default().to_owned()
}

fn is_chinese_region(code: &str) -> bool {
    matches!(code, "CN" | "HK" | "MO" | "TW")
}

static COUNTRY: Mutex<Weak<Reader<Vec<u8>>>> = Mutex::new(Weak::new());

/// Whether the decompressed country database is in memory.
pub fn country_database_loaded() -> bool {
    COUNTRY.lock().expect("country database lock").strong_count() > 0
}

/// The decompressed country database, kept only while someone uses it. It
/// takes about 12 MB, so an idle process should not hold it.
fn country_reader() -> Option<Arc<Reader<Vec<u8>>>> {
    let mut slot = COUNTRY.lock().expect("country database lock");
    if let Some(reader) = slot.upgrade() {
        return Some(reader);
    }
    let mut raw = Vec::with_capacity(12 << 20);
    xz2::read::XzDecoder::new(COUNTRY_XZ).read_to_end(&mut raw).ok()?;
    let reader = Arc::new(Reader::from_source(raw).ok()?);
    *slot = Arc::downgrade(&reader);
    Some(reader)
}

fn country_of(reader: &Reader<Vec<u8>>, ip: IpAddr) -> String {
    let Ok(result) = reader.lookup(ip) else { return String::new() };
    match result.decode::<CountryRecord>() {
        Ok(Some(rec)) => rec.country.iso_code.unwrap_or_default(),
        _ => String::new(),
    }
}

/// ISO code of the country of an address. Empty when the database does not
/// place it.
pub fn country_code(ip: IpAddr) -> String {
    country_reader().map(|r| country_of(&r, ip)).unwrap_or_default()
}

/// Locations of the addresses of the logs. One instance serves one indexing
/// round, then it is dropped so the country database and the mapped city
/// database are released. Neither is read before the first address is placed.
pub struct Geo {
    city: Option<Reader<Mmap>>,
    country: OnceLock<Option<Arc<Reader<Vec<u8>>>>>,
}

impl Geo {
    /// Opens the city database when `city_path` names a readable file.
    pub fn open(city_path: Option<&Path>) -> Arc<Geo> {
        let city = city_path.and_then(|p| {
            // SAFETY: the file is replaced by a rename when it is downloaded again, it is
            // never written in place, so the mapping stays valid.
            unsafe { Reader::open_mmap(p) }.ok()
        });
        Arc::new(Geo { city, country: OnceLock::new() })
    }

    /// A geo that only knows countries.
    pub fn countries_only() -> Arc<Geo> {
        Arc::new(Geo { city: None, country: OnceLock::new() })
    }

    pub fn has_city_database(&self) -> bool {
        self.city.is_some()
    }

    /// Places an address. `None` for text that is not an address.
    pub fn locate(&self, ip: &str) -> Option<GeoLocation> {
        let addr: IpAddr = ip.parse().ok()?;
        let country = self.country.get_or_init(country_reader).as_deref().map(|r| country_of(r, addr));
        let mut loc = GeoLocation { region_code: country.unwrap_or_default(), ..Default::default() };
        let Some(reader) = &self.city else {
            return (!loc.region_code.is_empty()).then_some(loc);
        };

        let record = match reader.lookup(addr).and_then(|r| r.decode::<CityRecord>()) {
            Ok(rec) => rec.unwrap_or_default(),
            Err(_) => return (!loc.region_code.is_empty()).then_some(loc),
        };
        let iso = trimmed(&record.country.iso_code);
        if loc.region_code.is_empty() {
            loc.region_code = iso.clone();
        }
        loc.c1 = trimmed(&record.c1);
        loc.c2 = trimmed(&record.c2);
        loc.c3 = trimmed(&record.c3);
        loc.c4 = trimmed(&record.c4);

        let sub = record.subdivisions.first();
        let province_en = first_non_empty([
            record.province.name.as_deref(),
            record.province.names.en.as_deref(),
            sub.and_then(|s| s.name.as_deref()),
            sub.and_then(|s| s.names.en.as_deref()),
        ]);
        let province_zh = first_non_empty([
            record.province.name_zh.as_deref(),
            record.province.names.zh_cn.as_deref(),
            sub.and_then(|s| s.name_zh.as_deref()),
            sub.and_then(|s| s.names.zh_cn.as_deref()),
        ]);
        let city_en = first_non_empty([record.city.name.as_deref(), record.city.names.en.as_deref()]);
        let city_zh = first_non_empty([record.city.name_zh.as_deref(), record.city.names.zh_cn.as_deref()]);

        loc.province = province_en;
        loc.city = city_en;
        if is_chinese_region(&loc.region_code) || is_chinese_region(&iso) {
            if !province_zh.is_empty() {
                loc.province = province_zh;
            } else if loc.province.is_empty() {
                loc.province = "其它".to_owned();
            }
            if !city_zh.is_empty() {
                loc.city = city_zh;
            }
            loc.region_code = "CN".to_owned();
        }
        Some(loc)
    }
}

/// Cache of the locations one thread looked up. Logs repeat their clients.
pub struct GeoCache {
    geo: Arc<Geo>,
    cache: HashMap<String, Option<GeoLocation>>,
}

const GEO_CACHE_LIMIT: usize = 10_000;

impl GeoCache {
    pub fn new(geo: Arc<Geo>) -> Self {
        Self { geo, cache: HashMap::new() }
    }

    pub fn locate(&mut self, ip: &str) -> Option<&GeoLocation> {
        if ip == "-" || ip.is_empty() {
            return None;
        }
        if !self.cache.contains_key(ip) {
            if self.cache.len() >= GEO_CACHE_LIMIT {
                self.cache.clear();
            }
            self.cache.insert(ip.to_owned(), self.geo.locate(ip));
        }
        self.cache.get(ip).and_then(Option::as_ref)
    }
}

/// Paths of the databases below the geolite directory.
#[derive(Debug, Clone)]
pub struct GeoPaths {
    dir: PathBuf,
    custom: String,
}

/// File name of the downloaded city database.
pub const CITY_DB_NAME: &str = "GeoLite2-City.mmdb";

impl GeoPaths {
    pub fn new(dir: PathBuf, custom: &str) -> Self {
        Self { dir, custom: custom.trim().to_owned() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The downloaded database, which a download replaces.
    pub fn default_db(&self) -> PathBuf {
        self.dir.join(CITY_DB_NAME)
    }

    pub fn default_xz(&self) -> PathBuf {
        self.dir.join(format!("{CITY_DB_NAME}.xz"))
    }

    /// The database in use: the downloaded one, otherwise the custom one.
    pub fn db_path(&self) -> PathBuf {
        let default = self.default_db();
        if default.exists() {
            return default;
        }
        if self.custom.is_empty() {
            return default;
        }
        let custom = Path::new(&self.custom);
        if custom.is_absolute() {
            custom.to_path_buf()
        } else {
            self.dir.join(custom)
        }
    }

    pub fn exists(&self) -> bool {
        self.db_path().is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn country_database_answers_from_the_embedded_copy() {
        assert_eq!(country_code("8.8.8.8".parse().unwrap()), "US");
        assert_eq!(country_code("127.0.0.1".parse().unwrap()), "");
    }

    #[test]
    fn countries_only_geo_places_public_addresses() {
        let geo = Geo::countries_only();
        assert_eq!(geo.locate("8.8.8.8").unwrap().region_code, "US");
        assert!(geo.locate("not an ip").is_none());
        assert!(geo.locate("10.0.0.1").is_none());
    }

    #[test]
    fn db_path_prefers_the_downloaded_database() {
        let dir = tempfile::tempdir().unwrap();
        let paths = GeoPaths::new(dir.path().to_path_buf(), "custom.mmdb");
        assert_eq!(paths.db_path(), dir.path().join("custom.mmdb"));
        std::fs::write(dir.path().join(CITY_DB_NAME), b"x").unwrap();
        assert_eq!(paths.db_path(), dir.path().join(CITY_DB_NAME));
        let abs = GeoPaths::new(dir.path().to_path_buf(), "/elsewhere/a.mmdb");
        std::fs::remove_file(dir.path().join(CITY_DB_NAME)).unwrap();
        assert_eq!(abs.db_path(), Path::new("/elsewhere/a.mmdb"));
    }
}
