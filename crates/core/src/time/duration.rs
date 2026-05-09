//! Duration formatter.
//!
//! `HH:MM:SS` 形式。`HH` は通算時数で、24 時間以上のセッションでも wrap しない
//! (`{total_hours:02}:{m:02}:{s:02}`)。

use chrono::Duration;

/// `HH:MM:SS` 表記。`HH` は通算時数で 24 を超えても wrap しない。
///
/// - 負の値 (left < joined) は `null` 相当として `--:--:--` を返す。
/// - 100 時間以上 (例: 99:59:59 超) はゼロ埋めなし、自然な桁数。
pub fn format_duration_hms(d: Duration) -> String {
    let total_secs = d.num_seconds();
    if total_secs < 0 {
        return "--:--:--".to_string();
    }
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_zero_duration() {
        assert_eq!(format_duration_hms(Duration::zero()), "00:00:00");
    }

    #[test]
    fn formats_under_one_hour() {
        assert_eq!(
            format_duration_hms(Duration::seconds(3 * 60 + 7)),
            "00:03:07"
        );
    }

    #[test]
    fn formats_one_hour_boundary() {
        assert_eq!(format_duration_hms(Duration::seconds(3600)), "01:00:00");
        assert_eq!(format_duration_hms(Duration::seconds(3599)), "00:59:59");
    }

    #[test]
    fn formats_just_under_24h() {
        assert_eq!(
            format_duration_hms(Duration::seconds(23 * 3600 + 59 * 60 + 59)),
            "23:59:59"
        );
    }

    #[test]
    fn formats_over_24h_without_wrap() {
        // 24 時間以上のセッションで hh:mm:ss が wrap しないこと。
        assert_eq!(
            format_duration_hms(Duration::seconds(25 * 3600 + 30 * 60)),
            "25:30:00"
        );
        assert_eq!(
            format_duration_hms(Duration::seconds(48 * 3600)),
            "48:00:00"
        );
    }

    #[test]
    fn formats_long_durations_without_zero_pad_when_three_digits() {
        // 100 時間以上は 3 桁出力 (Rust の format `02` は最小幅指定なので超過は許す)。
        assert_eq!(
            format_duration_hms(Duration::seconds(100 * 3600 + 5 * 60 + 9)),
            "100:05:09"
        );
        assert_eq!(
            format_duration_hms(Duration::seconds(999 * 3600)),
            "999:00:00"
        );
    }

    #[test]
    fn negative_duration_returns_placeholder() {
        // left < joined のような壊れたデータ。null 相当の placeholder を返す。
        assert_eq!(format_duration_hms(Duration::seconds(-1)), "--:--:--");
        assert_eq!(format_duration_hms(Duration::seconds(-3600)), "--:--:--");
    }
}
