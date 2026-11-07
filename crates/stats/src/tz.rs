use chrono::{NaiveDate, TimeZone};
use chrono_tz::America::New_York;

pub fn ms_to_date_key(ms: i64) -> String {
    match New_York.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%Y-%m-%d").to_string(),
        None => String::new(),
    }
}

pub fn day_start_ms(day: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let naive = date.and_hms_opt(0, 0, 0)?;
    New_York
        .from_local_datetime(&naive)
        .single()
        .or_else(|| New_York.from_local_datetime(&naive).earliest())
        .map(|dt| dt.timestamp_millis())
}

pub fn day_end_ms(day: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let next = date.succ_opt()?;
    day_start_ms(&next.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_key_uses_eastern_not_utc() {
        let ms = NaiveDate::from_ymd_opt(2024, 5, 4)
            .unwrap()
            .and_hms_opt(3, 30, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(ms_to_date_key(ms), "2024-05-03");
    }

    #[test]
    fn date_key_handles_the_dst_hour_that_was_wrong() {
        let ms = NaiveDate::from_ymd_opt(2024, 5, 4)
            .unwrap()
            .and_hms_opt(4, 30, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(ms_to_date_key(ms), "2024-05-04", "EDT is UTC-4, not UTC-5");

        let winter = NaiveDate::from_ymd_opt(2024, 12, 7)
            .unwrap()
            .and_hms_opt(4, 30, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(ms_to_date_key(winter), "2024-12-06", "EST is UTC-5");
    }

    #[test]
    fn day_start_offset_follows_the_season() {
        let summer = day_start_ms("2024-07-01").unwrap();
        let summer_utc = NaiveDate::from_ymd_opt(2024, 7, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(summer - summer_utc, 4 * 3600 * 1000, "EDT");

        let winter = day_start_ms("2024-01-01").unwrap();
        let winter_utc = NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(winter - winter_utc, 5 * 3600 * 1000, "EST");
    }

    #[test]
    fn day_bounds_round_trip() {
        for day in ["2023-02-22", "2024-05-04", "2024-11-03", "2026-09-16"] {
            let start = day_start_ms(day).expect(day);
            assert_eq!(ms_to_date_key(start), day, "midnight belongs to its own day");
            let end = day_end_ms(day).expect(day);
            assert_eq!(ms_to_date_key(end - 1), day, "the last millisecond still belongs to it");
            assert!(end > start);
        }
    }

    #[test]
    fn dst_days_are_not_all_twenty_four_hours() {
        let short = day_end_ms("2024-03-10").unwrap() - day_start_ms("2024-03-10").unwrap();
        let long = day_end_ms("2024-11-03").unwrap() - day_start_ms("2024-11-03").unwrap();
        assert_eq!(short, 23 * 3600 * 1000, "spring forward");
        assert_eq!(long, 25 * 3600 * 1000, "fall back");
    }

    #[test]
    fn day_start_rejects_nonsense() {
        assert!(day_start_ms("2026-13-01").is_none());
        assert!(day_start_ms("2026-02-30").is_none());
        assert!(day_start_ms("garbage").is_none());
        assert!(day_start_ms("").is_none());
    }
}
