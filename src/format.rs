//! Time and duration formatting.

use gtk::glib;

/// Current time as Unix seconds.
pub fn now() -> i64 {
    glib::real_time() / 1_000_000
}

/// `1:24:07`
pub fn hms(secs: i64) -> String {
    let secs = secs.max(0);
    format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
}

/// `1:24` (hours and minutes, rounded down)
pub fn hm(secs: i64) -> String {
    let minutes = secs.max(0) / 60;
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

fn local(ts: i64) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_local(ts).ok()
}

/// Local time of day, e.g. `09:12`.
pub fn clock_time(ts: i64) -> String {
    local(ts)
        .and_then(|dt| dt.format("%H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// Local calendar date, `YYYY-MM-DD`.
pub fn local_date(ts: i64) -> String {
    local(ts)
        .and_then(|dt| dt.format("%Y-%m-%d").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

pub fn today() -> String {
    local_date(now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(hms(0), "0:00:00");
        assert_eq!(hms(5_047), "1:24:07");
        assert_eq!(hms(-5), "0:00:00");
        assert_eq!(hms(36_000), "10:00:00");
        assert_eq!(hm(5_047), "1:24");
        assert_eq!(hm(59), "0:00");
        assert_eq!(hm(3_600), "1:00");
    }

    #[test]
    fn formats_local_dates() {
        let ts = 1_763_553_600; // 2025-11-19 12:00 UTC: same date in every timezone within ±11h
        assert_eq!(local_date(ts), "2025-11-19");
        assert_eq!(clock_time(ts).len(), 5);
    }
}
