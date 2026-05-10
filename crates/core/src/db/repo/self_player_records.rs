//! `self_player_records` repository。
//!
//! VRChat の `User Authenticated: <name>` ログ 1 行を 1 row として記録する。
//! `source_raw_event_id` UNIQUE で idempotent re-projection。
//!
//! 「現在の自分」を取りたい UI 側は [`fetch_latest`] を呼び、`authenticated_utc`
//! 降順の先頭 1 件を最新自己情報として扱う (アカウント切替で複数 row 残ることがある)。

use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::{Row, Sqlite, Transaction};

use super::world_visits::TimeContext;
use crate::time::resolve_local_to_utc;
use crate::Result;

/// 認証イベント 1 件を表す read shape。`fetch_latest` の戻り値。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfPlayerRecord {
    pub id: i64,
    pub display_name: String,
    pub authenticated_utc: DateTime<Utc>,
}

fn parse_tz(tz_id: &str) -> Result<chrono_tz::Tz> {
    tz_id
        .parse::<chrono_tz::Tz>()
        .map_err(|_| crate::Error::Config(format!("invalid tz: {tz_id}")))
}

/// `UserAuthenticated` イベントを 1 row として記録する。
/// `source_raw_event_id` UNIQUE で idempotent: 既存行があれば既存 id を返して終了。
pub async fn insert(
    tx: &mut Transaction<'_, Sqlite>,
    raw_id: i64,
    display_name: &str,
    naive_local: NaiveDateTime,
    ctx: TimeContext<'_>,
) -> Result<i64> {
    let existing: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM self_player_records WHERE source_raw_event_id = ?1")
            .bind(raw_id)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some((id,)) = existing {
        return Ok(id);
    }

    let tz = parse_tz(ctx.tz_id)?;
    let (utc, offset, res) = resolve_local_to_utc(naive_local, &tz, None);
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO self_player_records (
            source_raw_event_id, display_name,
            authenticated_naive_local, authenticated_utc,
            authenticated_tz_id, authenticated_offset_seconds,
            authenticated_resolution, authenticated_tz_source,
            authenticated_resolution_confidence
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        RETURNING id",
    )
    .bind(raw_id)
    .bind(display_name)
    .bind(naive_local.format("%Y-%m-%d %H:%M:%S").to_string())
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

/// `authenticated_utc` 最大 = 最新の認証イベントを 1 件返す。
/// 1 度も認証していない場合は `None`。
pub async fn fetch_latest(tx: &mut Transaction<'_, Sqlite>) -> Result<Option<SelfPlayerRecord>> {
    let row = sqlx::query(
        "SELECT id, display_name, authenticated_utc
         FROM self_player_records
         ORDER BY authenticated_utc DESC
         LIMIT 1",
    )
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => Ok(Some(SelfPlayerRecord {
            id: row.try_get("id")?,
            display_name: row.try_get("display_name")?,
            authenticated_utc: row.try_get("authenticated_utc")?,
        })),
        None => Ok(None),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::open;
    use crate::db::repo::processed_log_files::{self, ProcessedLogFileInput};
    use crate::db::repo::raw_log::{self, RawEventInput};
    use crate::log_parser::LogEvent;
    use chrono::NaiveDate;
    use tempfile::tempdir;

    async fn setup() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = open(&dir.path().join("test.db")).await.unwrap();
        (pool, dir)
    }

    fn ctx() -> TimeContext<'static> {
        TimeContext {
            tz_id: "Asia/Tokyo",
            tz_source: "CapturedRealtime",
            resolution_confidence: "High",
        }
    }

    fn nd(h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 5, 9)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    /// FK 用の processed_log_file + raw_log_event を 1 件 seed して raw_id を返す。
    async fn seed_raw(pool: &sqlx::SqlitePool, offset: i64, display_name: &str) -> i64 {
        let mut tx = pool.begin().await.unwrap();
        let pf_id = processed_log_files::upsert(
            &mut tx,
            &ProcessedLogFileInput {
                file_identity_hash: format!("h-{offset}"),
                log_sequence_key: format!("s-{offset}"),
                volume_serial: 0,
                file_id_high: 0,
                file_id_low: 0,
                generation: 0,
                creation_time: 0,
                first_kb_hash: "k".into(),
                file_name: "a.txt".into(),
                file_size: 0,
                mtime: chrono::Utc::now(),
                tz_id: "Asia/Tokyo".into(),
                tz_source: "CapturedRealtime".into(),
            },
        )
        .await
        .unwrap();
        let raw_id = raw_log::insert_batch_with_ledger(
            &mut tx,
            &[RawEventInput {
                processed_log_file_id: pf_id,
                byte_offset: offset,
                event: LogEvent::UserAuthenticated {
                    display_name: display_name.into(),
                },
                naive_local: Some(nd(20, 0)),
            }],
        )
        .await
        .unwrap()[0];
        tx.commit().await.unwrap();
        raw_id
    }

    #[tokio::test]
    async fn insert_creates_row_and_returns_positive_id() {
        let (pool, _dir) = setup().await;
        let raw_id = seed_raw(&pool, 100, "Alice").await;
        let mut tx = pool.begin().await.unwrap();

        let id = insert(&mut tx, raw_id, "Alice", nd(20, 0), ctx())
            .await
            .unwrap();

        assert!(id > 0);
    }

    #[tokio::test]
    async fn insert_is_idempotent_on_source_raw_event_id() {
        let (pool, _dir) = setup().await;
        let raw_id = seed_raw(&pool, 100, "Alice").await;
        let mut tx = pool.begin().await.unwrap();

        let first = insert(&mut tx, raw_id, "Alice", nd(20, 0), ctx())
            .await
            .unwrap();
        let second = insert(&mut tx, raw_id, "Alice", nd(20, 0), ctx())
            .await
            .unwrap();

        assert_eq!(first, second, "同 raw_id の二回目は既存 id を返す");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM self_player_records")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn fetch_latest_returns_none_for_clean_db() {
        let (pool, _dir) = setup().await;
        let mut tx = pool.begin().await.unwrap();

        let got = fetch_latest(&mut tx).await.unwrap();

        assert!(got.is_none());
    }

    #[tokio::test]
    async fn fetch_latest_returns_most_recent_row_when_multiple_exist() {
        // Arrange: 3 件の auth event を時刻バラバラに insert (アカウント切替シナリオ)
        let (pool, _dir) = setup().await;
        let raw1 = seed_raw(&pool, 100, "Old").await;
        let raw2 = seed_raw(&pool, 200, "Newest").await;
        let raw3 = seed_raw(&pool, 300, "Middle").await;
        let mut tx = pool.begin().await.unwrap();
        insert(&mut tx, raw1, "Old", nd(10, 0), ctx())
            .await
            .unwrap();
        insert(&mut tx, raw2, "Newest", nd(20, 0), ctx())
            .await
            .unwrap();
        insert(&mut tx, raw3, "Middle", nd(15, 0), ctx())
            .await
            .unwrap();

        // Act
        let got = fetch_latest(&mut tx).await.unwrap().unwrap();

        // Assert: authenticated_utc DESC で先頭 = "Newest"
        assert_eq!(got.display_name, "Newest");
    }
}
