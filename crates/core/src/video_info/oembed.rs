//! noembed.com を叩いて動画 metadata を取得する。
//!
//! noembed は YouTube / Vimeo / Twitch / SoundCloud 等の oEmbed プロバイダを
//! 統一インターフェイスでラップしたサービス。VRChat 内 video URL は YouTube
//! が支配的なので、まず noembed を叩いて埋まらない URL は negative cache 行き、
//! という単純運用で十分。
//!
//! - エンドポイント: `https://noembed.com/embed?url=<encoded>`
//! - 成功: `{"title": "...", "thumbnail_url": "...", "author_name": "...", ...}`
//! - 失敗: HTTP は 200 のまま `{"error": "..."}` を返してくることがある。
//!   `error` の有無を見て判定する。
//!
//! 設計上の注意:
//! - `OembedClient` は薄い `reqwest::Client` のラッパ。テストで mock 差し込みできるよう
//!   trait `OembedClientLike` を切らず、結果を検査する単発関数 `fetch_oembed` を
//!   `pub fn` で出して、上位 actor は `Client` を所有して呼び続ける形にしている。

use std::time::Duration;

use serde::Deserialize;

use crate::Result;

/// 1 リクエストの timeout。VRChat ログ処理を停滞させないため厳しめ。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// noembed の正常応答から actor が必要とする最小フィールドだけ拾う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OembedInfo {
    pub title: Option<String>,
    pub thumbnail_url: Option<String>,
}

/// reqwest::Client のラッパ。`new` で適切な User-Agent と timeout を設定する。
#[derive(Debug, Clone)]
pub struct OembedClient {
    inner: reqwest::Client,
}

impl OembedClient {
    pub fn new() -> Result<Self> {
        let inner = reqwest::Client::builder()
            .user_agent(concat!(
                "VRCWatchDog/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/kqnade/VRCWatchDog)"
            ))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| crate::Error::Config(format!("reqwest client build: {e}")))?;
        Ok(Self { inner })
    }
}

#[derive(Debug, Deserialize)]
struct NoembedResponse {
    title: Option<String>,
    thumbnail_url: Option<String>,
    error: Option<String>,
}

/// noembed.com に問い合わせる。
///
/// 戻り値:
/// - `Ok(Some(info))` : 成功。少なくとも title か thumbnail のどちらかが取れた。
/// - `Ok(None)` : noembed は応答したが結果が空 (provider non-supported)。negative cache 候補。
/// - `Err(_)` : ネットワーク / decode エラー。caller は retry を判断する。
pub async fn fetch_oembed(client: &OembedClient, url: &str) -> Result<Option<OembedInfo>> {
    let endpoint = format!(
        "https://noembed.com/embed?url={}",
        urlencoded::encode(url)
    );
    let resp = client
        .inner
        .get(&endpoint)
        .send()
        .await
        .map_err(|e| crate::Error::Config(format!("noembed GET: {e}")))?;
    if !resp.status().is_success() {
        return Err(crate::Error::Config(format!(
            "noembed returned status {}",
            resp.status()
        )));
    }
    let body: NoembedResponse = resp
        .json()
        .await
        .map_err(|e| crate::Error::Config(format!("noembed decode: {e}")))?;
    if body.error.is_some() {
        // noembed は不対応 provider に対して 200 + {"error": "..."} を返してくる。
        // negative cache の対象 (永続失敗扱い) としたいので Ok(None) を返す。
        return Ok(None);
    }
    let info = OembedInfo {
        title: body.title,
        thumbnail_url: body.thumbnail_url,
    };
    if info.title.is_none() && info.thumbnail_url.is_none() {
        Ok(None)
    } else {
        Ok(Some(info))
    }
}

/// パーセントエンコード用の最小実装。reqwest 内部に持っているが pub では出ていないので
/// 自前で軽く書く。`A-Za-z0-9_.-~` 以外は `%XX` でエスケープ。
mod urlencoded {
    pub fn encode(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        for b in input.as_bytes() {
            match *b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(*b as char)
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // urlencoded だけ unit test。HTTP fetch 側は mock サーバー無いと意味が無いので skip。

    #[test]
    fn urlencoded_encodes_special_characters_as_percent_hex() {
        let got = urlencoded::encode("https://example.com/v?a=1&b=2");
        assert_eq!(
            got,
            "https%3A%2F%2Fexample.com%2Fv%3Fa%3D1%26b%3D2",
            "scheme/colon/slash/?/=/& は全部 %XX 化"
        );
    }

    #[test]
    fn urlencoded_keeps_unreserved_characters_intact() {
        let got = urlencoded::encode("Abc-123_~.");
        assert_eq!(got, "Abc-123_~.");
    }

    #[test]
    fn oembed_client_builds_with_default_settings() {
        let _ = OembedClient::new().expect("default client must build");
    }
}
