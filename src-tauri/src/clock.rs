//! 极小的 RFC3339 UTC 时间格式化。
//!
//! 刻意不引入时间库：v0.1 只需要「当前时刻 → RFC3339 字符串」这一件事，
//! 而数据库侧的时间由 SQLite 的 `strftime` 负责。

use std::time::{SystemTime, UNIX_EPOCH};

/// Unix 秒 → `YYYY-MM-DDTHH:MM:SSZ`（UTC）。
pub fn format_rfc3339(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let seconds_of_day = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60
    )
}

/// 当前时刻（UTC）。
pub fn now_rfc3339() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    format_rfc3339(seconds)
}

/// Howard Hinnant 的 `civil_from_days`：1970-01-01 起的天数 → (年, 月, 日)。
///
/// `era` 一项里对负数做向下取整，因此 1970 年之前也正确（虽然本函数入参是 u64）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;

    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_unix_epoch() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn formats_last_second_of_the_day() {
        assert_eq!(format_rfc3339(86_399), "1970-01-01T23:59:59Z");
    }

    #[test]
    fn formats_a_known_recent_timestamp() {
        assert_eq!(format_rfc3339(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn handles_leap_day() {
        assert_eq!(format_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn handles_century_non_leap_year() {
        // 2100 不是闰年（能被 100 整除但无法被 400 整除），2 月只有 28 天。
        // 若算法错误地把 2100 当闰年，这两个断言之一就会失败。
        assert_eq!(format_rfc3339(4_107_456_000), "2100-02-28T00:00:00Z");
        assert_eq!(format_rfc3339(4_107_542_400), "2100-03-01T00:00:00Z");
    }

    #[test]
    fn now_is_rfc3339_shaped() {
        let now = now_rfc3339();

        assert_eq!(now.len(), 20, "期望 YYYY-MM-DDTHH:MM:SSZ，实际 {now}");
        assert!(now.ends_with('Z'), "必须以 Z 结尾：{now}");
        assert_eq!(&now[4..5], "-");
        assert_eq!(&now[10..11], "T");
        assert!(
            now[..4].chars().all(|c| c.is_ascii_digit()),
            "年份必须是数字：{now}"
        );
    }
}
