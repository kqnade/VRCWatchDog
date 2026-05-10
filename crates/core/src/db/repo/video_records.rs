//! `video_records` repository。
//!
//! `[Video Playback] ... Attempting to resolve URL '<url>'` 行を 1 行 insert する。
//! title / thumbnail は Phase 7 で `video_info` サービスから補完する。

use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::{Row, Sqlite, Transaction};

use super::world_visits::TimeContext;
use crate::time::resolve_local_to_utc;
use crate::Result;

/// `list_recent` の戻り値。/videos 画面用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoRecord {
    pub id: i64,
    pub url: String,
    pub title: Option<String>,
    pub thumbnail_url: Option<String>,
    pub thumbnail_sha: Option<String>,
    pub detected_naive_local: NaiveDateTime,
    pub detected_utc: DateTime<Utc>,
    pub world_visit_id: Option<i64>,
}

fn parse_tz(tz_id: &str) -> Result<chrono_tz::Tz> {
    tz_id
        .parse::<chrono_tz::Tz>()
        .map_err(|_| crate::Error::Config(format!("invalid tz: {tz_id}")))
}

fn fmt_naive(n: NaiveDateTime) -> String {
    n.format("%Y-%m-%d %H:%M:%S").to_string()
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    tx: &mut Transaction<'_, Sqlite>,
    raw_id: i64,
    world_visit_id: Option<i64>,
    url: &str,
    naive_local: NaiveDateTime,
    ctx: TimeContext<'_>,
) -> Result<i64> {
    let existing: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM video_records WHERE source_raw_event_id = ?1")
            .bind(raw_id)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some((id,)) = existing {
        return Ok(id);
    }

    let tz = parse_tz(ctx.tz_id)?;
    let (utc, offset, res) = resolve_local_to_utc(naive_local, &tz, None);
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO video_records (
            source_raw_event_id, world_visit_id, url,
            detected_naive_local, detected_utc,
            detected_tz_id, detected_offset_seconds,
            detected_resolution, detected_tz_source, detected_resolution_confidence
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        RETURNING id",
    )
    .bind(raw_id)
    .bind(world_visit_id)
    .bind(url)
    .bind(fmt_naive(naive_local))
    .bind(utc.to_rfc3339())
    .bind(ctx.tz_id)
    .bind(offset)
    .bind(res.as_str())
    .bind(ctx.tz_source)
    .bind(ctx.resolution_confidence)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

/// `title IS NULL` (= まだ video_info actor が触っていない) row を id 昇順で返す。
/// `limit <= 0` は空 vec。actor が batch 単位で取り出して順次 noembed に問い合わせる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMetadataRow {
    pub id: i64,
    pub url: String,
}

pub async fn list_pending_metadata(
    tx: &mut Transaction<'_, Sqlite>,
    limit: i64,
) -> Result<Vec<PendingMetadataRow>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT id, url FROM video_records
         WHERE title IS NULL
         ORDER BY id ASC
         LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(PendingMetadataRow {
            id: row.try_get("id")?,
            url: row.try_get("url")?,
        });
    }
    Ok(out)
}

/// 1 row の title / thumbnail_url / thumbnail_sha を更新する。
/// rows_affected を返す (= 0 なら id 不存在で no-op)。
pub async fn update_metadata(
    tx: &mut Transaction<'_, Sqlite>,
    id: i64,
    title: Option<&str>,
    thumbnail_url: Option<&str>,
    thumbnail_sha: Option<&str>,
) -> Result<u64> {
    let result = sqlx::query(
        "UPDATE video_records
         SET title = ?1, thumbnail_url = ?2, thumbnail_sha = ?3
         WHERE id = ?4",
    )
    .bind(title)
    .bind(thumbnail_url)
    .bind(thumbnail_sha)
    .bind(id)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected())
}

/// 検出日時 (`detected_utc`) の新しい順に最大 `limit` 件を返す。
pub async fn list_recent(tx: &mut Transaction<'_, Sqlite>, limit: i64) -> Result<Vec<VideoRecord>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT id, url, title, thumbnail_url, thumbnail_sha,
                detected_naive_local, detected_utc, world_visit_id
         FROM video_records
         ORDER BY detected_utc DESC
         LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let detected_naive_local_str: String = row.try_get("detected_naive_local")?;
        out.push(VideoRecord {
            id: row.try_get("id")?,
            url: row.try_get("url")?,
            title: row.try_get("title")?,
            thumbnail_url: row.try_get("thumbnail_url")?,
            thumbnail_sha: row.try_get("thumbnail_sha")?,
            detected_naive_local: NaiveDateTime::parse_from_str(
                &detected_naive_local_str,
                "%Y-%m-%d %H:%M:%S",
            )
            .map_err(|e| crate::Error::Config(format!("invalid detected_naive_local: {e}")))?,
            detected_utc: row.try_get("detected_utc")?,
            world_visit_id: row.try_get("world_visit_id")?,
        });
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::open;
    use chrono::NaiveDate;
    use tempfile::tempdir;

    /// FK 親 (processed_log_files + raw_log_events × N) を seed して raw_id を返す。
    async fn seed_pf_and_n_raw(pool: &sqlx::SqlitePool, count: usize) -> Vec<i64> {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO processed_log_files (
                file_identity_hash, log_sequence_key, volume_serial,
                file_id_high, file_id_low, generation, creation_time, first_kb_hash,
                file_name, file_size, mtime,
                ingest_position, last_projected_raw_event_id,
                tz_id, tz_source, processed_at
            ) VALUES (
                'h', '2026-05-10_00-00-00', 0, 0, 0, 0, 0, 'k',
                'a.txt', 0, '2026-05-10T00:00:00Z',
                0, NULL, 'Asia/Tokyo', 'CapturedRealtime', '2026-05-10T00:00:00Z'
            )",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        let pf_id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM processed_log_files")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let mut raw_ids = Vec::with_capacity(count);
        for offset in 0..count {
            sqlx::query(
                "INSERT INTO raw_log_events
                    (processed_log_file_id, byte_offset, event_type, payload_json)
                 VALUES (?1, ?2, 'VideoUrl', '{}')",
            )
            .bind(pf_id)
            .bind(offset as i64)
            .execute(&mut *tx)
            .await
            .unwrap();
            let id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM raw_log_events")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            raw_ids.push(id);
        }
        tx.commit().await.unwrap();
        raw_ids
    }

    fn ctx() -> TimeContext<'static> {
        TimeContext {
            tz_id: "Asia/Tokyo",
            tz_source: "CapturedRealtime",
            resolution_confidence: "High",
        }
    }

    fn nd(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    async fn fresh_pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = open(&dir.path().join("test.db")).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn list_recent_returns_empty_for_clean_db() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();

        let rows = list_recent(&mut tx, 100).await.unwrap();

        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn list_recent_orders_by_detected_utc_descending() {
        let (pool, _dir) = fresh_pool().await;
        let raws = seed_pf_and_n_raw(&pool, 3).await;
        let mut tx = pool.begin().await.unwrap();
        insert(
            &mut tx,
            raws[0],
            None,
            "https://example.com/middle",
            nd(2026, 5, 10, 12, 0, 0),
            ctx(),
        )
        .await
        .unwrap();
        insert(
            &mut tx,
            raws[1],
            None,
            "https://example.com/oldest",
            nd(2026, 5, 9, 8, 0, 0),
            ctx(),
        )
        .await
        .unwrap();
        insert(
            &mut tx,
            raws[2],
            None,
            "https://example.com/newest",
            nd(2026, 5, 10, 18, 0, 0),
            ctx(),
        )
        .await
        .unwrap();

        let rows = list_recent(&mut tx, 10).await.unwrap();

        let urls: Vec<_> = rows.iter().map(|v| v.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://example.com/newest",
                "https://example.com/middle",
                "https://example.com/oldest",
            ]
        );
    }

    #[tokio::test]
    async fn list_recent_returns_title_and_thumbnail_as_none_until_video_info_filled() {
        // Phase 7.3.3 の video_info service が後で埋める前提なので、insert 直後は
        // title / thumbnail_url / thumbnail_sha が全て None になることを確認する。
        let (pool, _dir) = fresh_pool().await;
        let raws = seed_pf_and_n_raw(&pool, 1).await;
        let mut tx = pool.begin().await.unwrap();
        insert(
            &mut tx,
            raws[0],
            None,
            "https://example.com/v",
            nd(2026, 5, 10, 12, 0, 0),
            ctx(),
        )
        .await
        .unwrap();

        let rows = list_recent(&mut tx, 10).await.unwrap();

        assert_eq!(rows.len(), 1);
        assert!(rows[0].title.is_none());
        assert!(rows[0].thumbnail_url.is_none());
        assert!(rows[0].thumbnail_sha.is_none());
    }

    #[tokio::test]
    async fn list_recent_returns_empty_when_limit_is_zero_or_negative() {
        let (pool, _dir) = fresh_pool().await;
        let raws = seed_pf_and_n_raw(&pool, 1).await;
        let mut tx = pool.begin().await.unwrap();
        insert(
            &mut tx,
            raws[0],
            None,
            "u",
            nd(2026, 5, 10, 12, 0, 0),
            ctx(),
        )
        .await
        .unwrap();

        let zero = list_recent(&mut tx, 0).await.unwrap();
        let neg = list_recent(&mut tx, -1).await.unwrap();

        assert!(zero.is_empty());
        assert!(neg.is_empty());
    }
}
