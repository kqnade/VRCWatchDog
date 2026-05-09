use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;

/// VRChat 標準ログ名: `output_log_2026-05-09_21-43-56.txt`
static RE_LOG_FILENAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^output_log_(\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2})\.txt$").expect("RE_LOG_FILENAME")
});

/// `processed_log_files.log_sequence_key` を導出する。
///
/// VRChat 標準ファイル名から `yyyy-MM-dd_HH-mm-ss` を抽出するのが第一優先。
/// 抽出失敗時は creation_time を ISO8601 (UTC) で代替する。
/// この値は projection 順序の sort key として使われるので、
/// **同一フォーマット内での辞書順 = 時系列順** が保証されればよい。
pub fn derive_log_sequence_key(path: &Path, creation_time_utc: DateTime<Utc>) -> String {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if let Some(caps) = RE_LOG_FILENAME.captures(name) {
            return caps[1].to_string();
        }
    }
    // フォールバック: creation_time を ISO8601 にして辞書順 = 時系列順を確保。
    creation_time_utc.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn ut(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, s).unwrap()
    }

    #[test]
    fn extracts_from_standard_vrchat_filename() {
        let key = derive_log_sequence_key(
            &p("/some/dir/output_log_2026-05-09_21-43-56.txt"),
            ut(2030, 1, 1, 0, 0, 0),
        );
        assert_eq!(key, "2026-05-09_21-43-56");
    }

    #[test]
    fn falls_back_to_creation_time_when_filename_unmatched() {
        let key =
            derive_log_sequence_key(&p("/some/dir/random_name.log"), ut(2026, 5, 9, 12, 0, 0));
        assert_eq!(key, "2026-05-09T12:00:00Z");
    }

    #[test]
    fn falls_back_when_path_has_no_filename() {
        // ディレクトリ末尾だけの path
        let key = derive_log_sequence_key(&p("/"), ut(2026, 5, 9, 12, 0, 0));
        assert_eq!(key, "2026-05-09T12:00:00Z");
    }

    #[test]
    fn standard_filenames_sort_chronologically() {
        let mut keys = vec![
            derive_log_sequence_key(
                &p("output_log_2026-05-09_21-43-56.txt"),
                ut(2030, 1, 1, 0, 0, 0),
            ),
            derive_log_sequence_key(
                &p("output_log_2026-05-08_10-00-00.txt"),
                ut(2030, 1, 1, 0, 0, 0),
            ),
            derive_log_sequence_key(
                &p("output_log_2026-05-09_22-00-00.txt"),
                ut(2030, 1, 1, 0, 0, 0),
            ),
        ];
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "2026-05-08_10-00-00",
                "2026-05-09_21-43-56",
                "2026-05-09_22-00-00",
            ]
        );
    }
}
