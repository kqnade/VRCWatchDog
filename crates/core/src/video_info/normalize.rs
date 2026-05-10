//! URL 正規化。同一動画の表記ゆれ (tracking, fragment, host case) を吸収して
//! `failed_video_lookups` のキャッシュキーやログ重複検出に使う。
//!
//! YouTube / youtu.be は別 host 同一動画なので、video_id を抽出して `youtube://<id>`
//! という人工 URL に統一する。それ以外のホストは host 小文字化 + tracking param 除去
//! のみ実施し、相対順は保つ (sort はしない — 同じパラメータ列なら同じ key)。
//!
//! `NORMALIZATION_VERSION` は normalize ロジックを変更したときに失効させたい
//! negative cache 行を判別するための version 文字列 (`failed_video_lookups`
//! テーブルの `normalization_version` 列と紐づく)。

use std::collections::BTreeSet;

/// 正規化ロジックの version。`failed_video_lookups.normalization_version` に保存し、
/// 後で normalize ルールを変えたら古い row を `version != current` で無効化できる。
pub const NORMALIZATION_VERSION: &str = "v1";

/// `NORMALIZATION_VERSION` 文字列の typed wrapper (lib 外から触るのは reference 型で十分)。
pub type NormalizationVersion = &'static str;

/// 同一動画と見做すべき URL を 1 つの canonical 文字列に潰す。
///
/// 戻り値は API ヒット用の生 URL ではなく、**比較・キャッシュキー専用** の正規化文字列。
/// noembed への問い合わせは元の URL を使う。
pub fn normalize_url(input: &str) -> String {
    let trimmed = input.trim();

    // YouTube は host が `(www.|m.|music.)?youtube.com` (`/watch?v=ID`) と
    // `youtu.be/ID` の 2 形態がある。video_id を抽出して統一表現にする。
    if let Some(yt_id) = extract_youtube_video_id(trimmed) {
        return format!("youtube://{yt_id}");
    }

    // 一般 URL: lowercase scheme+host、tracking params 除去、fragment 除去。
    // url crate を増やしたくないので軽く手書き。失敗したら入力そのまま返す
    // (= negative cache に raw URL がそのまま入るが冪等性は保たれる)。
    let Some((scheme_host, rest)) = split_scheme_host(trimmed) else {
        return trimmed.to_string();
    };
    let lower_prefix = scheme_host.to_ascii_lowercase();
    // rest = path + ? + query + # + fragment
    let (path_query, _frag) = rest.split_once('#').unwrap_or((rest, ""));
    let (path, query) = match path_query.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path_query, None),
    };
    let cleaned_query = query.map(strip_tracking_params).filter(|s| !s.is_empty());
    match cleaned_query {
        Some(q) => format!("{lower_prefix}{path}?{q}"),
        None => format!("{lower_prefix}{path}"),
    }
}

/// `https://www.youtube.com/watch?v=XYZ&...` / `https://youtu.be/XYZ` 等から
/// 11 文字の video ID を取り出す (見つからなければ None)。
fn extract_youtube_video_id(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    // youtu.be/ID 形式
    if let Some(rest) = lower.strip_prefix("https://youtu.be/") {
        return Some(read_yt_id_from_path(rest));
    }
    if let Some(rest) = lower.strip_prefix("http://youtu.be/") {
        return Some(read_yt_id_from_path(rest));
    }
    // youtube.com/watch?v=ID 形式 (host prefix の www / m / music を許容)
    let yt_host_starts = [
        "https://www.youtube.com/",
        "https://m.youtube.com/",
        "https://music.youtube.com/",
        "https://youtube.com/",
        "http://www.youtube.com/",
        "http://m.youtube.com/",
        "http://music.youtube.com/",
        "http://youtube.com/",
    ];
    let mut after_host = None;
    for prefix in yt_host_starts {
        if let Some(rest) = lower.strip_prefix(prefix) {
            after_host = Some(rest);
            break;
        }
    }
    let after_host = after_host?;
    // /watch?v=ID
    if let Some(query) = after_host.strip_prefix("watch?") {
        for pair in query.split('&') {
            if let Some(id) = pair.strip_prefix("v=") {
                let id = id.split(&['&', '#'][..]).next().unwrap_or(id);
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
    }
    // /shorts/ID
    if let Some(rest) = after_host.strip_prefix("shorts/") {
        return Some(read_yt_id_from_path(rest));
    }
    // /embed/ID
    if let Some(rest) = after_host.strip_prefix("embed/") {
        return Some(read_yt_id_from_path(rest));
    }
    None
}

/// `<id>?si=...&t=10` 形式の path 部から先頭の id 部分のみ取り出す。
fn read_yt_id_from_path(rest: &str) -> String {
    let id = rest.split(&['/', '?', '#'][..]).next().unwrap_or(rest);
    id.to_string()
}

/// `https://example.com/path?q=1` を `("https://example.com", "/path?q=1")` に分ける。
/// scheme://host/ が見つからなければ None。
fn split_scheme_host(url: &str) -> Option<(&str, &str)> {
    let scheme_end = url.find("://")?;
    let after_scheme = scheme_end + 3;
    let path_start_offset = url[after_scheme..].find('/')?;
    let split_at = after_scheme + path_start_offset;
    Some(url.split_at(split_at))
}

/// クエリ文字列から utm_* / gclid / fbclid 等の tracking パラメータを取り除く。
/// 並び順は保つ (同入力なら同出力)。
fn strip_tracking_params(query: &str) -> String {
    static TRACKING_PREFIXES: &[&str] = &[
        "utm_", "gclid", "fbclid", "mc_cid", "mc_eid", "yclid", "_hsenc", "_hsmi",
    ];
    let mut keep: Vec<&str> = Vec::new();
    let mut seen = BTreeSet::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let key = pair.split('=').next().unwrap_or(pair).to_ascii_lowercase();
        let is_tracking = TRACKING_PREFIXES
            .iter()
            .any(|p| key.starts_with(*p) || key == *p);
        if is_tracking {
            continue;
        }
        // duplicate key 同一値は 1 度だけ残す (noembed 側に余計なクエリを渡さない)
        if seen.insert(pair.to_string()) {
            keep.push(pair);
        }
    }
    keep.join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- YouTube canonicalization ---------------------------------------------

    #[test]
    fn normalize_collapses_youtu_be_and_youtube_com_to_same_canonical_form() {
        let a = normalize_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        let b = normalize_url("https://youtu.be/dQw4w9WgXcQ");

        assert_eq!(a, b);
        assert_eq!(a, "youtube://dqw4w9wgxcq");
    }

    #[test]
    fn normalize_strips_youtube_tracking_params_si_and_t_keeping_only_video_id() {
        let got = normalize_url("https://youtu.be/abc123?si=xyz&t=42");
        assert_eq!(got, "youtube://abc123");
    }

    #[test]
    fn normalize_recognizes_shorts_and_embed_youtube_paths() {
        let shorts = normalize_url("https://www.youtube.com/shorts/abc123def");
        let embed = normalize_url("https://www.youtube.com/embed/abc123def");

        assert_eq!(shorts, "youtube://abc123def");
        assert_eq!(embed, "youtube://abc123def");
    }

    // -- generic URL ----------------------------------------------------------

    #[test]
    fn normalize_lowercases_scheme_and_host_for_non_youtube_urls() {
        let got = normalize_url("HTTPS://Example.COM/PATH?Q=1");
        // path / query のケースは保つ (/PATH と Q=1 が残る)
        assert_eq!(got, "https://example.com/PATH?Q=1");
    }

    #[test]
    fn normalize_strips_utm_and_other_tracking_params() {
        let got =
            normalize_url("https://example.com/v?id=42&utm_source=x&utm_medium=y&gclid=z&keep=1");
        assert_eq!(got, "https://example.com/v?id=42&keep=1");
    }

    #[test]
    fn normalize_drops_fragment_after_hash() {
        let got = normalize_url("https://example.com/v?id=1#bookmark");
        assert_eq!(got, "https://example.com/v?id=1");
    }

    #[test]
    fn normalize_keeps_input_unchanged_when_url_has_no_scheme() {
        // 解析失敗時の最後の砦。同じ raw 文字列に対して同じ key になればそれで十分。
        let got = normalize_url("not-a-url");
        assert_eq!(got, "not-a-url");
    }

    // -- determinism ----------------------------------------------------------

    #[test]
    fn normalize_is_deterministic_when_called_twice_on_same_input() {
        let input = "https://example.com/X?a=1&b=2&utm_x=drop";
        assert_eq!(normalize_url(input), normalize_url(input));
    }
}
