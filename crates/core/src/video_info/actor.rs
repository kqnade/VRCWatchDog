//! VideoInfoActor: `title IS NULL` の video_records を順次補完する poll loop。
//!
//! 動作概要 (1 tick):
//! 1. `list_pending_metadata` で N 件取り出す
//! 2. 各 row について:
//!    - normalize_url → sha256 hex を作る
//!    - failed_video_lookups で active な negative cache があれば skip
//!    - noembed に問い合わせる
//!    - 成功 (Some) → thumbnail を download + webp 化 + thumb_sha 計算 →
//!      video_records の title / thumbnail_url / thumbnail_sha を更新
//!    - 失敗 (None: provider 非対応) → negative cache に 7 日 TTL で行を立てる
//!    - 失敗 (Err: 一時エラー) → negative cache に 5 分 TTL で行を立てる
//!    - 各リクエスト間に `request_interval` だけ sleep して noembed を叩きすぎない
//! 3. `poll_interval` 待って次 tick
//!
//! 設計上の選択:
//! - thumbnail 保存は thumb_writer と同じ `<thumb_dir>/<sha>.webp` に統一する。
//!   asset:// scope (`$LOCALAPPDATA/.../thumbs/**`) でそのまま表示できる。
//! - 1 video あたり 1 transaction (重い HTTP は tx 外で実行 → tx は短く)。
//! - 致命エラー (DB 障害) で loop を止めない: 1 件失敗時は warn + 次の row へ。

use std::path::PathBuf;
use std::time::Duration;

use blake3;
use chrono::Utc;
use image::ImageEncoder;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::db::repo::{failed_video_lookups, video_records};
use crate::video_info::normalize::{normalize_url, NORMALIZATION_VERSION};
use crate::video_info::oembed::{fetch_oembed, OembedClient, OembedInfo};
use crate::Result;

/// VideoInfoActor の設定。
#[derive(Debug, Clone)]
pub struct VideoInfoConfig {
    /// 1 tick で fetch する pending row 数。
    pub batch_size: i64,
    /// tick 間隔。
    pub poll_interval: Duration,
    /// 同 batch 内のリクエスト間 sleep。noembed への礼儀。
    pub request_interval: Duration,
    /// 永続失敗 (provider 非対応など) のキャッシュ TTL。
    pub negative_ttl_permanent: chrono::Duration,
    /// 一時失敗 (network) のキャッシュ TTL。次の tick より少し長め。
    pub negative_ttl_transient: chrono::Duration,
    /// `<thumb_dir>/<sha>.webp` を書く先。photo の thumb_writer と共有する。
    pub thumb_dir: PathBuf,
}

impl VideoInfoConfig {
    pub fn new(thumb_dir: PathBuf) -> Self {
        Self {
            batch_size: 5,
            poll_interval: Duration::from_secs(15),
            request_interval: Duration::from_millis(700),
            negative_ttl_permanent: chrono::Duration::days(7),
            negative_ttl_transient: chrono::Duration::minutes(10),
            thumb_dir,
        }
    }
}

/// 状態を持たない (= configurable poll loop)。bootstrap が `tokio::spawn(actor.run())` する。
pub struct VideoInfoActor {
    pool: SqlitePool,
    client: OembedClient,
    config: VideoInfoConfig,
}

impl VideoInfoActor {
    pub fn new(pool: SqlitePool, config: VideoInfoConfig) -> Result<Self> {
        let client = OembedClient::new()?;
        Ok(Self {
            pool,
            client,
            config,
        })
    }

    /// 1 tick を実行する。返り値は actor 自体のテスト可能性のため (process_one_batch
    /// は loop 内 only でも書けるが、テスト時に `await actor.tick()` できるよう pub)。
    pub async fn tick(&self) -> Result<TickStats> {
        let mut tx = self.pool.begin().await?;
        let pending = video_records::list_pending_metadata(&mut tx, self.config.batch_size).await?;
        tx.commit().await?;
        if pending.is_empty() {
            return Ok(TickStats::default());
        }

        let mut stats = TickStats {
            considered: pending.len(),
            ..Default::default()
        };

        for (i, row) in pending.iter().enumerate() {
            // 1 件目以外は request_interval だけ sleep (rate limit)
            if i > 0 {
                tokio::time::sleep(self.config.request_interval).await;
            }

            let normalized = normalize_url(&row.url);
            let key_sha = sha256_hex(&normalized);

            // negative cache check (短い tx)
            let now = Utc::now();
            let is_cached = {
                let mut tx = self.pool.begin().await?;
                let active = failed_video_lookups::is_active(&mut tx, &key_sha, now).await?;
                tx.commit().await?;
                active
            };
            if is_cached {
                stats.cached_skip += 1;
                continue;
            }

            match fetch_oembed(&self.client, &row.url).await {
                Ok(Some(info)) => {
                    let thumbnail_sha = match info.thumbnail_url.as_deref() {
                        Some(url) => self
                            .download_and_store_thumbnail(url)
                            .await
                            .inspect_err(|e| {
                                tracing::warn!(video_id = row.id, error = %e, "thumbnail download failed (record without thumb_sha)");
                            })
                            .ok(),
                        None => None,
                    };
                    let mut tx = self.pool.begin().await?;
                    let updated = video_records::update_metadata(
                        &mut tx,
                        row.id,
                        info.title.as_deref(),
                        info.thumbnail_url.as_deref(),
                        thumbnail_sha.as_deref(),
                    )
                    .await?;
                    tx.commit().await?;
                    if updated > 0 {
                        stats.updated += 1;
                        record_success(row.id, &info, thumbnail_sha.is_some());
                    }
                }
                Ok(None) => {
                    let expires = now + self.config.negative_ttl_permanent;
                    self.cache_negative(
                        &key_sha,
                        &row.url,
                        expires,
                        "permanent",
                        "noembed: empty/unsupported",
                    )
                    .await
                    .ok();
                    stats.permanent_fail += 1;
                }
                Err(e) => {
                    let expires = now + self.config.negative_ttl_transient;
                    let reason = e.to_string();
                    self.cache_negative(&key_sha, &row.url, expires, "transient", &reason)
                        .await
                        .ok();
                    stats.transient_fail += 1;
                    tracing::debug!(video_id = row.id, error = %reason, "video_info transient fail");
                }
            }
        }

        Ok(stats)
    }

    /// `pool` を消費せず loop 化。stop は `tokio::JoinSet::abort` で。
    pub async fn run(self) {
        loop {
            match self.tick().await {
                Ok(s) if s.considered > 0 => {
                    tracing::info!(
                        considered = s.considered,
                        updated = s.updated,
                        cached_skip = s.cached_skip,
                        permanent_fail = s.permanent_fail,
                        transient_fail = s.transient_fail,
                        "video_info tick",
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "video_info tick failed"),
            }
            tokio::time::sleep(self.config.poll_interval).await;
        }
    }

    async fn cache_negative(
        &self,
        key_sha: &str,
        raw_url: &str,
        expires_at: chrono::DateTime<chrono::Utc>,
        status: &str,
        reason: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        failed_video_lookups::upsert(
            &mut tx,
            key_sha,
            raw_url,
            NORMALIZATION_VERSION,
            expires_at,
            status,
            reason,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// thumbnail_url から bytes を fetch → image::load → webp encode →
    /// `<thumb_dir>/<sha>.webp` に書き、blake3 hex を返す。
    async fn download_and_store_thumbnail(&self, thumbnail_url: &str) -> Result<String> {
        let resp = self
            .client_get(thumbnail_url)
            .await
            .map_err(|e| crate::Error::Config(format!("thumb GET: {e}")))?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| crate::Error::Config(format!("thumb body: {e}")))?;

        // image crate で decode → webp encode (lossless 不要なので default lossy WebP)
        let img = image::load_from_memory(&bytes)
            .map_err(|e| crate::Error::Config(format!("thumb decode: {e}")))?;
        let mut webp_bytes: Vec<u8> = Vec::new();
        let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut webp_bytes);
        encoder
            .write_image(
                img.to_rgba8().as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| crate::Error::Config(format!("thumb encode: {e}")))?;
        let sha = blake3::hash(&webp_bytes).to_hex().to_string();
        let path = self.config.thumb_dir.join(format!("{sha}.webp"));
        // 同 sha なら既存 (誰かの thumbnail と blake3 衝突 = 同 bytes) なので overwrite OK
        if !path.exists() {
            tokio::fs::create_dir_all(&self.config.thumb_dir)
                .await
                .map_err(|e| crate::Error::Config(format!("thumb mkdir: {e}")))?;
            tokio::fs::write(&path, &webp_bytes)
                .await
                .map_err(|e| crate::Error::Config(format!("thumb write: {e}")))?;
        }
        Ok(sha)
    }

    async fn client_get(&self, url: &str) -> reqwest::Result<reqwest::Response> {
        // OembedClient 内部 reqwest::Client を使い回したいが pub では出していないので
        // 別 Client を使う。reqwest::Client は cheap clone (Arc inner) で構わない。
        // 共有化したいときは OembedClient に `pub fn inner()` を追加する。
        reqwest::Client::builder()
            .timeout(crate::video_info::oembed::REQUEST_TIMEOUT)
            .build()?
            .get(url)
            .send()
            .await
    }
}

/// `tick` の結果サマリ。test と log 用。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickStats {
    /// 1 tick で取り出した pending row 数。
    pub considered: usize,
    /// title / thumbnail を埋めた件数。
    pub updated: usize,
    /// negative cache が active で skip した件数。
    pub cached_skip: usize,
    /// 永続失敗 (provider 非対応) として cache 行を立てた件数。
    pub permanent_fail: usize,
    /// 一時失敗 (network 等) として cache 行を立てた件数。
    pub transient_fail: usize,
}

fn record_success(video_id: i64, info: &OembedInfo, thumb_stored: bool) {
    tracing::info!(
        video_id,
        has_title = info.title.is_some(),
        has_thumbnail_url = info.thumbnail_url.is_some(),
        thumb_stored,
        "video_info hit",
    );
}

fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    let digest = h.finalize();
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_is_deterministic_and_64_chars_long() {
        let a = sha256_hex("youtube://abc123");
        let b = sha256_hex("youtube://abc123");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn sha256_hex_differs_for_different_inputs() {
        let a = sha256_hex("youtube://abc");
        let b = sha256_hex("youtube://xyz");
        assert_ne!(a, b);
    }

    #[test]
    fn config_new_uses_sensible_defaults() {
        let cfg = VideoInfoConfig::new(PathBuf::from("/tmp/thumbs"));
        assert_eq!(cfg.batch_size, 5);
        assert_eq!(cfg.poll_interval, Duration::from_secs(15));
        assert!(cfg.request_interval >= Duration::from_millis(500));
        assert!(cfg.negative_ttl_permanent >= chrono::Duration::days(1));
    }
}
