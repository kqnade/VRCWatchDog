//! VRChat の screenshot ファイル名 parser。
//!
//! VRChat は撮影時に `VRChat_YYYY-MM-DD_HH-MM-SS.fff_WIDTHxHEIGHT.<ext>` という
//! 命名規則でファイルを保存する。EXIF を読むより安く、欠落しないので、撮影時刻と
//! 解像度はこのファイル名から取り出すのが標準アプローチ。
//!
//! サポートする variant:
//! - 標準形: `VRChat_2026-05-10_12-34-56.789_1920x1080.png`
//! - 古いビルド (fractional 秒なし): `VRChat_2026-05-10_12-34-56_1920x1080.png`
//! - 解像度なし (極めて稀): `VRChat_2026-05-10_12-34-56.png`
//! - 拡張子: `.png` / `.jpg` / `.jpeg` (case-insensitive)
//!
//! 上記 variant 以外はすべて `None` を返す。
//! VRChat 以外のスクリーンショットも同 dir に置かれることがあるが、それらは
//! 取り込み対象外として明示的に reject する (誤った taken_naive_local を入れない)。

use std::sync::OnceLock;

use chrono::NaiveDateTime;
use regex::Regex;

/// `parse_vrchat_filename` の戻り値。撮影時刻と (取れれば) 解像度を持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VRChatPhotoMeta {
    pub taken_naive_local: NaiveDateTime,
    /// `WIDTHxHEIGHT` を取れた場合のみ。古いビルドや手動リネームで欠落することがある。
    pub resolution: Option<(u32, u32)>,
}

/// VRChat 命名規則のファイル名から [`VRChatPhotoMeta`] を取り出す。
///
/// 入力にディレクトリパスは含めず、純粋に **ファイル名部分のみ** を渡すこと
/// (PathBuf ユーザーは `path.file_name()?.to_str()?` で先に抽出する)。
pub fn parse_vrchat_filename(name: &str) -> Option<VRChatPhotoMeta> {
    let caps = vrchat_re().captures(name)?;
    let date_str = caps.name("date")?.as_str();
    let time_str = caps.name("time")?.as_str();

    // 秒以下のフラクションは結合してから一括 parse する。
    // chrono の `%S%.f` は `12-34-56.789` ではなく `12:34:56.789` を期待するので、
    // 区切り文字を一旦 `:` に直す。
    let normalized_time = time_str.replace('-', ":");
    let combined = format!("{date_str} {normalized_time}");
    let taken_naive_local = if combined.contains('.') {
        NaiveDateTime::parse_from_str(&combined, "%Y-%m-%d %H:%M:%S%.f").ok()?
    } else {
        NaiveDateTime::parse_from_str(&combined, "%Y-%m-%d %H:%M:%S").ok()?
    };

    let resolution = match (caps.name("w"), caps.name("h")) {
        (Some(w), Some(h)) => {
            let w: u32 = w.as_str().parse().ok()?;
            let h: u32 = h.as_str().parse().ok()?;
            Some((w, h))
        }
        _ => None,
    };

    Some(VRChatPhotoMeta {
        taken_naive_local,
        resolution,
    })
}

/// 1 度だけコンパイルしてプロセス全体で使い回す。
fn vrchat_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // 解説:
        // - `^VRChat_` プレフィックス必須
        // - `date`: YYYY-MM-DD
        // - `time`: HH-MM-SS、fractional `.fff` は optional
        // - resolution `_WIDTHxHEIGHT` は optional
        // - 拡張子は png/jpg/jpeg (case-insensitive、`(?i)` で囲む)
        Regex::new(
            r"(?xi)
            ^VRChat_
            (?P<date>\d{4}-\d{2}-\d{2})
            _
            (?P<time>\d{2}-\d{2}-\d{2}(?:\.\d+)?)
            (?:_(?P<w>\d+)x(?P<h>\d+))?
            \.(?:png|jpg|jpeg)$
            ",
        )
        .expect("vrchat filename regex must compile")
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn nd(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    fn nd_milli(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32, milli: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_milli_opt(h, mi, s, milli)
            .unwrap()
    }

    // -- happy path: 標準形 --------------------------------------------------------

    #[test]
    fn parses_standard_filename_with_fractional_seconds_and_resolution() {
        let got = parse_vrchat_filename("VRChat_2026-05-10_12-34-56.789_1920x1080.png");

        assert_eq!(
            got,
            Some(VRChatPhotoMeta {
                taken_naive_local: nd_milli(2026, 5, 10, 12, 34, 56, 789),
                resolution: Some((1920, 1080)),
            })
        );
    }

    // -- variants ----------------------------------------------------------------

    #[test]
    fn parses_filename_without_fractional_seconds() {
        // 古いビルドや手動リネーム想定
        let got = parse_vrchat_filename("VRChat_2026-05-10_12-34-56_1920x1080.png");

        assert_eq!(
            got,
            Some(VRChatPhotoMeta {
                taken_naive_local: nd(2026, 5, 10, 12, 34, 56),
                resolution: Some((1920, 1080)),
            })
        );
    }

    #[test]
    fn parses_filename_without_resolution_segment() {
        // 解像度欠落 (リネーム / 古いビルド) でも撮影時刻だけは取り出す。
        let got = parse_vrchat_filename("VRChat_2026-05-10_12-34-56.png");

        assert_eq!(
            got,
            Some(VRChatPhotoMeta {
                taken_naive_local: nd(2026, 5, 10, 12, 34, 56),
                resolution: None,
            })
        );
    }

    #[test]
    fn parses_jpg_and_jpeg_extensions_in_addition_to_png() {
        let png = parse_vrchat_filename("VRChat_2026-05-10_12-34-56.png");
        let jpg = parse_vrchat_filename("VRChat_2026-05-10_12-34-56.jpg");
        let jpeg = parse_vrchat_filename("VRChat_2026-05-10_12-34-56.jpeg");

        assert!(png.is_some());
        assert!(jpg.is_some());
        assert!(jpeg.is_some());
    }

    #[test]
    fn parses_uppercase_extension_case_insensitively() {
        // ユーザーが手動リネームで `.PNG` にした想定。
        let got = parse_vrchat_filename("VRChat_2026-05-10_12-34-56.PNG");

        assert!(got.is_some());
    }

    // -- reject cases --------------------------------------------------------------

    #[test]
    fn rejects_non_vrchat_prefix_filename() {
        // 同 dir に他アプリのスクリーンショットが置かれた場合。
        let got = parse_vrchat_filename("Discord_2026-05-10_12-34-56.png");

        assert!(got.is_none());
    }

    #[test]
    fn rejects_empty_string() {
        assert!(parse_vrchat_filename("").is_none());
    }

    #[test]
    fn rejects_filename_with_path_separators() {
        // ディレクトリパス込みで渡された場合 (caller の bug 防衛)。
        // `^VRChat_` で始まらないので reject される。
        let got = parse_vrchat_filename("C:/Users/Foo/VRChat_2026-05-10_12-34-56.png");
        assert!(
            got.is_none(),
            "ディレクトリ込みでは reject、`file_name()` で抽出せよ"
        );
    }

    #[test]
    fn rejects_invalid_date_components() {
        // 13 月、32 日、25 時など。regex は数字パターンだけ見るので一旦通るが、
        // chrono の parse_from_str が None を返す。
        let got = parse_vrchat_filename("VRChat_2026-13-99_12-34-56.png");

        assert!(got.is_none());
    }

    #[test]
    fn rejects_partial_match_inside_longer_name() {
        // 中途半端に suffix が付いている例。`$` アンカーで reject する。
        let got = parse_vrchat_filename("VRChat_2026-05-10_12-34-56.png.bak");

        assert!(got.is_none());
    }

    #[test]
    fn rejects_tmp_extension_used_by_inflight_writes() {
        // VRChat が write 中に作る `.tmp` は scan 対象外なので reject。
        // (実際は scanner 側で extension filter する想定だが、parser 単体でも safety net)
        let got = parse_vrchat_filename("VRChat_2026-05-10_12-34-56.tmp");

        assert!(got.is_none());
    }
}
