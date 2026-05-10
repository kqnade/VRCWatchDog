//! `failed_video_lookups` repository。
//!
//! `video_info` actor が noembed への問い合わせに失敗したとき、同じ URL を
//! 短時間で再試行しないように TTL 付きで negative cache 行を立てる。
//!
//! - PK は `normalized_key_sha` (= [`normalize_url`] 結果の sha256 hex)。同 URL の
//!   表記ゆれを吸収して同一 key になる。
//! - `expires_at` は UTC ISO8601。`is_active` で「現在時刻 < expires_at」を判定。
//! - `normalization_version` 列で normalize ロジック改版時に invalidate できる。
//!
//! [`normalize_url`]: crate::video_info::normalize::normalize_url

use chrono::{DateTime, Utc};
use sqlx::{Sqlite, Transaction};

use crate::Result;

/// `key_sha` の negative cache 行が **現在時刻時点で有効** か。
/// 行が無ければ false、`expires_at <= now` でも false (caller が再 fetch して上書き)。
pub async fn is_active(
    tx: &mut Transaction<'_, Sqlite>,
    key_sha: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT expires_at FROM failed_video_lookups WHERE normalized_key_sha = ?1",
    )
    .bind(key_sha)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((expires_at_str,)) = row else {
        return Ok(false);
    };
    let expires_at: DateTime<Utc> = match DateTime::parse_from_rfc3339(&expires_at_str) {
        Ok(t) => t.with_timezone(&Utc),
        Err(_) => return Ok(false), // 不正 → 失効扱いで再 fetch を許す
    };
    Ok(expires_at > now)
}

/// `key_sha` PK で 1 行 upsert。同 key の既存行があれば全列更新 (TTL 延長)。
pub async fn upsert(
    tx: &mut Transaction<'_, Sqlite>,
    key_sha: &str,
    raw_url: &str,
    normalization_version: &str,
    expires_at: DateTime<Utc>,
    status: &str,
    reason: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO failed_video_lookups (
            normalized_key_sha, raw_url, normalization_version,
            expires_at, status, reason
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(normalized_key_sha) DO UPDATE SET
            raw_url = excluded.raw_url,
            normalization_version = excluded.normalization_version,
            expires_at = excluded.expires_at,
            status = excluded.status,
            reason = excluded.reason",
    )
    .bind(key_sha)
    .bind(raw_url)
    .bind(normalization_version)
    .bind(expires_at.to_rfc3339())
    .bind(status)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::open;
    use chrono::Duration;
    use tempfile::tempdir;

    async fn fresh_pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = open(&dir.path().join("test.db")).await.unwrap();
        (pool, dir)
    }

    fn now_fixed() -> DateTime<Utc> {
        chrono::TimeZone::from_utc_datetime(
            &Utc,
            &chrono::NaiveDate::from_ymd_opt(2026, 5, 10)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn is_active_returns_false_for_unknown_key() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();

        let active = is_active(&mut tx, "nonexistent", now_fixed()).await.unwrap();

        assert!(!active);
    }

    #[tokio::test]
    async fn is_active_returns_true_when_expires_at_is_in_future() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        upsert(
            &mut tx,
            "k1",
            "https://x.example/v",
            "v1",
            now_fixed() + Duration::hours(24),
            "permanent",
            "noembed: unsupported provider",
        )
        .await
        .unwrap();

        let active = is_active(&mut tx, "k1", now_fixed()).await.unwrap();

        assert!(active);
    }

    #[tokio::test]
    async fn is_active_returns_false_when_expires_at_is_already_past() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        upsert(
            &mut tx,
            "k2",
            "https://x.example/v",
            "v1",
            now_fixed() - Duration::seconds(1),
            "permanent",
            "expired",
        )
        .await
        .unwrap();

        let active = is_active(&mut tx, "k2", now_fixed()).await.unwrap();

        assert!(!active, "TTL 過ぎたら caller に再 fetch を促す");
    }

    #[tokio::test]
    async fn upsert_replaces_existing_row_on_same_key_with_new_ttl() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        upsert(
            &mut tx,
            "k3",
            "url-old",
            "v1",
            now_fixed() + Duration::seconds(60),
            "transient",
            "old",
        )
        .await
        .unwrap();
        upsert(
            &mut tx,
            "k3",
            "url-new",
            "v1",
            now_fixed() + Duration::hours(48),
            "permanent",
            "new",
        )
        .await
        .unwrap();

        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT raw_url, status, reason FROM failed_video_lookups WHERE normalized_key_sha = 'k3'",
        )
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        assert_eq!(rows.len(), 1, "PK 1 件のまま");
        assert_eq!(rows[0].0, "url-new");
        assert_eq!(rows[0].1, "permanent");
        assert_eq!(rows[0].2, "new");
    }
}
