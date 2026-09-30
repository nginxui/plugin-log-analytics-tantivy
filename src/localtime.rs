//! Server local calendar helpers for the dashboard buckets.
//!
//! The daily buckets follow the local time zone of the server, like the
//! existing Go plugin, so these helpers mirror what `time.AddDate` and
//! `time.Format` do there.

use chrono::{DateTime, Datelike, Duration, Local, LocalResult, NaiveDateTime, TimeZone};

fn local(ts: i64) -> DateTime<Local> {
    Local.timestamp_opt(ts, 0).single().unwrap_or_else(|| Local.timestamp_opt(0, 0).unwrap())
}

/// Resolves a local wall clock time, stepping over a daylight saving gap.
fn resolve(naive: NaiveDateTime) -> i64 {
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(t) => t.timestamp(),
        LocalResult::Ambiguous(first, _) => first.timestamp(),
        LocalResult::None => match Local.from_local_datetime(&(naive + Duration::hours(1))) {
            LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => t.timestamp(),
            LocalResult::None => naive.and_utc().timestamp(),
        },
    }
}

/// Timestamp of a wall clock time in the local zone, `None` for an invalid
/// date. Error logs of nginx write their times this way.
pub fn from_local(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Option<i64> {
    let naive = chrono::NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(hour, minute, second)?;
    Some(resolve(naive))
}

/// Local calendar date of a timestamp as `year * 10000 + month * 100 + day`.
pub fn date_key(ts: i64) -> i32 {
    let d = local(ts);
    d.year() * 10000 + (d.month() as i32) * 100 + d.day() as i32
}

/// The same wall clock time on the next local calendar day.
pub fn add_local_day(ts: i64) -> i64 {
    resolve(local(ts).naive_local() + Duration::days(1))
}

/// First instant of the local calendar day that contains `ts`.
pub fn local_midnight(ts: i64) -> i64 {
    let naive = local(ts).date_naive().and_hms_opt(0, 0, 0).expect("midnight");
    resolve(naive)
}

/// Local date of a timestamp as `YYYY-MM-DD`.
pub fn format_date(ts: i64) -> String {
    local(ts).format("%Y-%m-%d").to_string()
}

/// A timestamp as RFC 3339 in the local zone, like Go's `time.RFC3339`.
pub fn format_rfc3339(ts: i64) -> String {
    local(ts).to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_steps_and_keys_agree() {
        let ts = 1_790_000_000;
        let next = add_local_day(ts);
        assert!(next - ts >= 23 * 3600 && next - ts <= 25 * 3600);
        assert_ne!(date_key(ts), date_key(next));
        assert!(local_midnight(ts) <= ts);
        assert_eq!(date_key(local_midnight(ts)), date_key(ts));
        assert_eq!(format_date(ts).len(), 10);
    }
}
