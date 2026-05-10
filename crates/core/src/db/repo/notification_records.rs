//! `notification_records` repository。
//!
//! `Received Notification` 行を 1 行 insert する。`source_raw_event_id` UNIQUE で
//! idempotent。

use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::{Row, Sqlite, Transaction};

use super::world_visits::TimeContext;
use crate::time::resolve_local_to_utc;
use crate::Result;

/// `list_recent` の戻り値。/notifications 画面用。
///
/// 詳細 tz フィールド (offset/resolution/tz_source/confidence) は UI で要らないので
/// 省く (photo_records::PhotoRecord と同じ方針)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRecord {
    pub id: i64,
    pub received_naive_local: NaiveDateTime,
    pub received_utc: DateTime<Utc>,
    pub sender_name: String,
    pub notification_type: String,
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
    sender_name: &str,
    notification_type: &str,
    naive_local: NaiveDateTime,
    ctx: TimeContext<'_>,
) -> Result<i64> {
    let existing: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM notification_records WHERE source_raw_event_id = ?1")
            .bind(raw_id)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some((id,)) = existing {
        return Ok(id);
    }

    let tz = parse_tz(ctx.tz_id)?;
    let (utc, offset, res) = resolve_local_to_utc(naive_local, &tz, None);

    let row: (i64,) = sqlx::query_as(
        "INSERT INTO notification_records (
            source_raw_event_id, world_visit_id,
            received_naive_local, received_utc,
            received_tz_id, received_offset_seconds,
            received_resolution, received_tz_source, received_resolution_confidence,
            sender_name, notification_type
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        RETURNING id",
    )
    .bind(raw_id)
    .bind(world_visit_id)
    .bind(fmt_naive(naive_local))
    .bind(utc.to_rfc3339())
    .bind(ctx.tz_id)
    .bind(offset)
    .bind(res.as_str())
    .bind(ctx.tz_source)
    .bind(ctx.resolution_confidence)
    .bind(sender_name)
    .bind(notification_type)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

/// 受信日時 (`received_utc`) の新しい順に最大 `limit` 件を返す。
///
/// `limit <= 0` で空 vec (他 repo helpers と揃えた防衛)。
pub async fn list_recent(
    tx: &mut Transaction<'_, Sqlite>,
    limit: i64,
) -> Result<Vec<NotificationRecord>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT id, received_naive_local, received_utc,
                sender_name, notification_type, world_visit_id
         FROM notification_records
         ORDER BY received_utc DESC
         LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let received_naive_local_str: String = row.try_get("received_naive_local")?;
        out.push(NotificationRecord {
            id: row.try_get("id")?,
            received_naive_local: NaiveDateTime::parse_from_str(
                &received_naive_local_str,
                "%Y-%m-%d %H:%M:%S",
            )
            .map_err(|e| crate::Error::Config(format!("invalid received_naive_local: {e}")))?,
            received_utc: row.try_get("received_utc")?,
            sender_name: row.try_get("sender_name")?,
            notification_type: row.try_get("notification_type")?,
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
    use chrono::{NaiveDate, TimeZone};
    use tempfile::tempdir;

    /// notification_records は raw_log_events を FK 参照するので、最小親行を seed する。
    /// pool ごとの 1 度きり。
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
                 VALUES (?1, ?2, 'Notification', '{}')",
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

    fn _utc(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.from_utc_datetime(&nd(y, m, d, h, mi, s))
    }

    async fn fresh_pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = open(&dir.path().join("test.db")).await.unwrap();
        (pool, dir)
    }

    // -- list_recent --------------------------------------------------------

    #[tokio::test]
    async fn list_recent_returns_empty_for_clean_db() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();

        let rows = list_recent(&mut tx, 100).await.unwrap();

        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn list_recent_orders_by_received_utc_descending() {
        // Arrange: 3 件、insert 順がバラバラの時刻
        let (pool, _dir) = fresh_pool().await;
        let raws = seed_pf_and_n_raw(&pool, 3).await;
        let mut tx = pool.begin().await.unwrap();
        let _mid = insert(
            &mut tx,
            raws[0],
            None,
            "Mid",
            "invite",
            nd(2026, 5, 10, 12, 0, 0),
            ctx(),
        )
        .await
        .unwrap();
        let _old = insert(
            &mut tx,
            raws[1],
            None,
            "Old",
            "invite",
            nd(2026, 5, 9, 8, 0, 0),
            ctx(),
        )
        .await
        .unwrap();
        let _new = insert(
            &mut tx,
            raws[2],
            None,
            "New",
            "invite",
            nd(2026, 5, 10, 18, 0, 0),
            ctx(),
        )
        .await
        .unwrap();

        let rows = list_recent(&mut tx, 10).await.unwrap();

        let names: Vec<_> = rows.iter().map(|n| n.sender_name.as_str()).collect();
        assert_eq!(names, vec!["New", "Mid", "Old"]);
    }

    #[tokio::test]
    async fn list_recent_respects_limit() {
        let (pool, _dir) = fresh_pool().await;
        let raws = seed_pf_and_n_raw(&pool, 5).await;
        let mut tx = pool.begin().await.unwrap();
        for (i, raw_id) in raws.iter().enumerate() {
            insert(
                &mut tx,
                *raw_id,
                None,
                &format!("user{i}"),
                "invite",
                nd(2026, 5, 10, 10 + i as u32, 0, 0),
                ctx(),
            )
            .await
            .unwrap();
        }

        let rows = list_recent(&mut tx, 2).await.unwrap();

        assert_eq!(rows.len(), 2);
        let names: Vec<_> = rows.iter().map(|n| n.sender_name.as_str()).collect();
        assert_eq!(names, vec!["user4", "user3"], "新しい 2 件");
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
            "invite",
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
