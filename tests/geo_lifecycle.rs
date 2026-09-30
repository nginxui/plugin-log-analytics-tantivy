//! The country database is read when the first address is placed and freed
//! with the geo that used it. This test has a process of its own, so no other
//! test holds the database while it looks.

use plugin_log_analytics_rs::geo::{country_database_loaded, Geo};

#[test]
fn the_country_database_is_read_on_first_use_and_freed_with_its_geo() {
    assert!(!country_database_loaded());
    let geo = Geo::countries_only();
    assert!(!country_database_loaded(), "opening a geo reads nothing");
    assert_eq!(geo.locate("8.8.8.8").unwrap().region_code, "US");
    assert!(country_database_loaded());
    let second = Geo::countries_only();
    assert_eq!(second.locate("8.8.4.4").unwrap().region_code, "US");
    drop(geo);
    assert!(country_database_loaded(), "the second geo still holds it");
    drop(second);
    assert!(!country_database_loaded());
}
