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
    /// The province and city in English, or in Chinese when the database has
    /// no English name. The page shows other languages, see `city_id`.
    pub province: String,
    pub city: String,
    /// GeoNames id of the city, 0 when the database has none. The page looks
    /// up the name of the city in its language by it.
    pub city_id: u64,
    /// ISO 3166-2 codes of the first two subdivision levels, like `US-CA` or
    /// `FR-IDF` and `FR-75`. Empty when the database has none.
    pub sub1: String,
    pub sub2: String,
    /// The city for the hotspot map, see [`city_point`]. Empty without
    /// coordinates.
    pub city_point: String,
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
    iso_code: Option<String>,
    // The database stores it as uint32, which the decoder reads only into a u32
    geoname_id: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Coordinates {
    latitude: Option<f64>,
    longitude: Option<f64>,
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
    location: Coordinates,
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

        let country = if iso.is_empty() { loc.region_code.clone() } else { iso.clone() };
        let code_of = |place: Option<&Place>| {
            place
                .and_then(|p| p.iso_code.as_deref())
                .map(str::trim)
                .filter(|c| !c.is_empty() && !country.is_empty())
                .map(|c| format!("{country}-{c}"))
                .unwrap_or_default()
        };
        loc.sub1 = code_of(record.subdivisions.first());
        loc.sub2 = code_of(record.subdivisions.get(1));

        loc.province = if province_en.is_empty() { province_zh } else { province_en };
        loc.city = if city_en.is_empty() { city_zh } else { city_en };
        loc.city_id = record.city.geoname_id.map(u64::from).unwrap_or_default();
        if is_chinese_region(&loc.region_code) || is_chinese_region(&iso) {
            // Hong Kong, Macau and Taiwan are regions of the China map
            if country != "CN" && is_chinese_region(&country) {
                loc.sub1 = format!("CN-{country}");
                loc.sub2.clear();
            }
            loc.region_code = "CN".to_owned();
        }
        if let (Some(lat), Some(lon)) = (record.location.latitude, record.location.longitude) {
            if !loc.city.is_empty() && lat.is_finite() && lon.is_finite() {
                loc.city_point = city_point(&loc.region_code, &loc.city, loc.city_id, lat, lon);
            }
        }
        Some(loc)
    }
}

/// The hotspot key of a city: `country|name|latitude|longitude|id`, the
/// coordinates rounded to two decimals and the GeoNames id left out when the
/// database has none. A `|` in the name would split the key, so it is replaced.
/// The Go plugin writes the same keys.
pub fn city_point(country: &str, city: &str, id: u64, lat: f64, lon: f64) -> String {
    let key = format!("{country}|{}|{lat:.2}|{lon:.2}", city.replace('|', "/"));
    if id == 0 {
        key
    } else {
        format!("{key}|{id}")
    }
}

/// A hotspot key split into country, name, GeoNames id (0 without one),
/// latitude and longitude. `None` for text in another form.
pub fn parse_city_point(key: &str) -> Option<(&str, &str, u64, f64, f64)> {
    let parts: Vec<&str> = key.split('|').collect();
    let id = match parts.len() {
        4 => 0,
        5 => parts[4].parse().ok()?,
        _ => return None,
    };
    Some((parts[0], parts[1], id, parts[2].parse().ok()?, parts[3].parse().ok()?))
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
    fn a_place_reads_its_geonames_id() {
        // The decoder refuses a uint32 for any other integer type, and a failed
        // field fails the whole record. The country of the embedded database
        // is a place like the city of the city database.
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Record {
            country: Place,
        }
        let reader = country_reader().unwrap();
        let record: Record = reader.lookup("8.8.8.8".parse().unwrap()).unwrap().decode().unwrap().unwrap();
        assert_eq!(record.country.geoname_id, Some(6252001));
    }

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

    #[test]
    fn city_points_round_trip() {
        let key = city_point("US", "Salt Lake|City", 0, 40.7608, -111.8910);
        assert_eq!(key, "US|Salt Lake/City|40.76|-111.89");
        assert_eq!(parse_city_point(&key), Some(("US", "Salt Lake/City", 0, 40.76, -111.89)));
        let key = city_point("US", "Tampa", 4174757, 27.9475, -82.4584);
        assert_eq!(key, "US|Tampa|27.95|-82.46|4174757");
        assert_eq!(parse_city_point(&key), Some(("US", "Tampa", 4174757, 27.95, -82.46)));
        assert_eq!(parse_city_point("broken"), None);
    }
}
