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

pub fn date_string(year: i32, month: u32, day: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

/// Splits `YYYY-MM-DD` into year, month and day.
pub fn date_parts(date: &str) -> Option<(i32, u32, u32)> {
    let mut parts = date.splitn(3, '-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    Some((year, month, day))
}

fn local_day(date: &str) -> Option<glib::DateTime> {
    let (y, m, d) = date_parts(date)?;
    glib::DateTime::from_local(y, m as i32, d as i32, 12, 0, 0.0).ok()
}

fn format_day(date: &str, pattern: &str) -> String {
    local_day(date)
        .and_then(|dt| dt.format(pattern).ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| date.to_string())
}

/// `Tue 17 Nov`
pub fn short_date(date: &str) -> String {
    format_day(date, "%a %-d %b")
}

/// `Tuesday 17 November`
pub fn long_date(date: &str) -> String {
    format_day(date, "%A %-d %B")
}

/// `November 2026`. Capitalized, as some languages write month names in
/// lowercase but this is used as a title.
pub fn month_label(year: i32, month: u32) -> String {
    capitalize(&format_day(&date_string(year, month, 1), "%B %Y"))
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `Nov`
pub fn short_month(year: i32, month: u32) -> String {
    format_day(&date_string(year, month, 1), "%b")
}

/// Unix time of `hour:minute` local time on `date`.
pub fn local_timestamp(date: &str, hour: u32, minute: u32) -> Option<i64> {
    let (y, m, d) = date_parts(date)?;
    glib::DateTime::from_local(y, m as i32, d as i32, hour as i32, minute as i32, 0.0)
        .ok()
        .map(|dt| dt.to_unix())
}

/// Parses a time of day: `9:05`, `09.05`, `905`, `9`.
pub fn parse_clock(text: &str) -> Option<(u32, u32)> {
    let text = text.trim();
    let (hours, minutes) = match text.split_once([':', '.']) {
        Some((h, m)) => (h, m),
        None if text.len() > 2 => text.split_at(text.len() - 2),
        None => (text, "0"),
    };
    let hours: u32 = hours.trim().parse().ok()?;
    let minutes: u32 = minutes.trim().parse().ok()?;
    (hours < 24 && minutes < 60).then_some((hours, minutes))
}

/// Parses a duration in seconds: `1:30`, `1.5`, `1,5`, `90m`, `1h30`,
/// `1h 30m`, or a bare number of hours (`2`).
pub fn parse_duration(text: &str) -> Option<i64> {
    let text: String = text.trim().to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let number = |s: &str| -> Option<f64> {
        if s.is_empty() {
            return Some(0.0);
        }
        s.replace(',', ".").parse::<f64>().ok().filter(|v| *v >= 0.0)
    };
    let secs = if let Some((h, m)) = text.split_once(':') {
        let minutes = m.parse::<u32>().ok().filter(|m| *m < 60)?;
        number(h)? * 3600.0 + f64::from(minutes) * 60.0
    } else if let Some((h, m)) = text.split_once('h') {
        number(h)? * 3600.0 + number(m.trim_end_matches('m'))? * 60.0
    } else if let Some(m) = text.strip_suffix('m') {
        number(m)? * 60.0
    } else {
        number(&text)? * 3600.0
    };
    let secs = secs.round() as i64;
    (secs > 0 && !text.is_empty()).then_some(secs)
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
    fn parses_clock_times() {
        assert_eq!(parse_clock("9:05"), Some((9, 5)));
        assert_eq!(parse_clock("09.05"), Some((9, 5)));
        assert_eq!(parse_clock("905"), Some((9, 5)));
        assert_eq!(parse_clock("1430"), Some((14, 30)));
        assert_eq!(parse_clock("9"), Some((9, 0)));
        assert_eq!(parse_clock(" 23:59 "), Some((23, 59)));
        assert_eq!(parse_clock("24:00"), None);
        assert_eq!(parse_clock("9:60"), None);
        assert_eq!(parse_clock("abc"), None);
        assert_eq!(parse_clock(""), None);
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("1:30"), Some(5_400));
        assert_eq!(parse_duration("0:45"), Some(2_700));
        assert_eq!(parse_duration("1.5"), Some(5_400));
        assert_eq!(parse_duration("1,5"), Some(5_400));
        assert_eq!(parse_duration("90m"), Some(5_400));
        assert_eq!(parse_duration("1h30"), Some(5_400));
        assert_eq!(parse_duration("1h 30m"), Some(5_400));
        assert_eq!(parse_duration("2h"), Some(7_200));
        assert_eq!(parse_duration("2"), Some(7_200));
        assert_eq!(parse_duration("3.48"), Some(12_528));
        assert_eq!(parse_duration("0"), None);
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("1:75"), None);
        assert_eq!(parse_duration("-1"), None);
        assert_eq!(parse_duration("abc"), None);
    }

    #[test]
    fn capitalizes_titles() {
        assert_eq!(capitalize("november 2026"), "November 2026");
        assert_eq!(capitalize("éte"), "Éte");
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn handles_dates() {
        assert_eq!(date_parts("2026-11-17"), Some((2026, 11, 17)));
        assert_eq!(date_string(2026, 3, 4), "2026-03-04");
        assert_eq!(date_parts("nope"), None);
        let start = local_timestamp("2026-11-17", 9, 0).unwrap();
        let end = local_timestamp("2026-11-17", 10, 30).unwrap();
        assert_eq!(end - start, 5_400);
        assert_eq!(local_date(start), "2026-11-17");
    }

    #[test]
    fn formats_local_dates() {
        let ts = 1_763_553_600; // 2025-11-19 12:00 UTC: same date in every timezone within ±11h
        assert_eq!(local_date(ts), "2025-11-19");
        assert_eq!(clock_time(ts).len(), 5);
    }
}
