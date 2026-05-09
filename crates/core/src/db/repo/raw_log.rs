//! `raw_log_events` + `projected_raw_events` の同時 insert。
//!
//! 設計の中核 (Phase 4a 不変条件):
//!
//! 1. **raw insert と ledger `Pending` insert は必ず同一 transaction で行う**。
//!    raw だけ存在して ledger に無い「幽霊行」を作らない。
//! 2. raw `(processed_log_file_id, byte_offset)` は UNIQUE。同 raw を 2 回流しても
//!    `ON CONFLICT DO NOTHING` で既存 id を取得し、ledger は補修 insert される。
//! 3. ingest_position 更新も同一 tx に含めるため、呼び出し側は本関数の後に
//!    [`super::processed_log_files::set_ingest_position`] を**同じ tx 内で**呼ぶこと。

use chrono::{NaiveDateTime, Utc};
use sqlx::{Sqlite, Transaction};

use crate::log_parser::LogEvent;
use crate::Result;

/// 1 ログ行を raw 永続化するための入力。
#[derive(Debug, Clone)]
pub struct RawEventInput {
    pub processed_log_file_id: i64,
    pub byte_offset: i64,
    pub event: LogEvent,
    pub naive_local: Option<NaiveDateTime>,
}

/// raw_log_events と projected_raw_events を同一 tx で insert する。
///
/// 既存 raw (UNIQUE 衝突) の場合は既存 id を取得し、ledger も
/// `ON CONFLICT DO NOTHING` で補修するため、再実行しても重複は発生しない。
///
/// 戻り値は入力順に対応する raw event id のベクタ。
pub async fn insert_batch_with_ledger(
    tx: &mut Transaction<'_, Sqlite>,
    batch: &[RawEventInput],
) -> Result<Vec<i64>> {
    let now = Utc::now().to_rfc3339();
    let mut ids = Vec::with_capacity(batch.len());

    for input in batch {
        let payload_json = serde_json::to_string(&input.event)?;
        let event_type = input.event.type_tag();
        let naive_local_str = input
            .naive_local
            .map(|n| n.format("%Y-%m-%d %H:%M:%S").to_string());

        // 1. raw insert (UNIQUE 衝突なら既存 id を返す)
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO raw_log_events (
                processed_log_file_id, byte_offset, event_type, payload_json, naive_local
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(processed_log_file_id, byte_offset) DO UPDATE SET id = id
            RETURNING id",
        )
        .bind(input.processed_log_file_id)
        .bind(input.byte_offset)
        .bind(event_type)
        .bind(&payload_json)
        .bind(naive_local_str.as_deref())
        .fetch_one(&mut **tx)
        .await?;
        let raw_id = row.0;

        // 2. ledger Pending insert (重複時は既存を尊重)
        sqlx::query(
            "INSERT INTO projected_raw_events (raw_event_id, status, created_at, updated_at)
             VALUES (?1, 'Pending', ?2, ?2)
             ON CONFLICT(raw_event_id) DO NOTHING",
        )
        .bind(raw_id)
        .bind(&now)
        .execute(&mut **tx)
        .await?;

        ids.push(raw_id);
    }

    Ok(ids)
}

/// 起動時 recovery 用: ledger 行を持たない raw 行 (orphan) の件数を返す。
///
/// `count` > 0 は ingest が ledger と異なる tx で書かれたバグの兆候。
/// 自動補修するか fatal エラーにするかは呼び出し側 (`super::write_actor`) で判定する。
pub async fn count_orphan_raw_without_ledger(tx: &mut Transaction<'_, Sqlite>) -> Result<i64> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM raw_log_events r
         LEFT JOIN projected_raw_events p ON p.raw_event_id = r.id
         WHERE p.raw_event_id IS NULL",
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

/// orphan raw に対して `Pending` ledger を補修 insert する。
/// 呼び出し側は事前に件数を [`count_orphan_raw_without_ledger`] で確認し、
/// 閾値 (例: 1000 件) を超える場合は補修せず `FatalCorruption` 扱いにすること。
pub async fn repair_orphan_ledger(tx: &mut Transaction<'_, Sqlite>) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        "INSERT INTO projected_raw_events (raw_event_id, status, created_at, updated_at)
         SELECT r.id, 'Pending', ?1, ?1
         FROM raw_log_events r
         LEFT JOIN projected_raw_events p ON p.raw_event_id = r.id
         WHERE p.raw_event_id IS NULL",
    )
    .bind(&now)
    .execute(&mut **tx)
    .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::processed_log_files::{
        self, get_ingest_position, set_ingest_position, ProcessedLogFileInput,
    };
    use super::*;
    use crate::db::open;
    use chrono::NaiveDate;
    use sqlx::Row;
    use tempfile::tempdir;

    async fn fresh_pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let pool = open(&path).await.unwrap();
        (pool, dir)
    }

    fn pf_input(hash: &str, name: &str) -> ProcessedLogFileInput {
        ProcessedLogFileInput {
            file_identity_hash: hash.into(),
            log_sequence_key: "2026-05-09_00-00-00".into(),
            volume_serial: 0,
            file_id_high: 0,
            file_id_low: 0,
            generation: 0,
            creation_time: 0,
            first_kb_hash: "k1".into(),
            file_name: name.into(),
            file_size: 0,
            mtime: Utc::now(),
            tz_id: "Asia/Tokyo".into(),
            tz_source: "CapturedRealtime".into(),
        }
    }

    fn raw_input(file_id: i64, byte_offset: i64, world_name: &str) -> RawEventInput {
        RawEventInput {
            processed_log_file_id: file_id,
            byte_offset,
            event: LogEvent::RoomEntering {
                world_name: world_name.into(),
            },
            naive_local: Some(
                NaiveDate::from_ymd_opt(2026, 5, 9)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
            ),
        }
    }

    /// 不変条件: raw insert は必ず ledger Pending を同時に作る。
    #[tokio::test]
    async fn raw_and_ledger_inserted_atomically() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf_id = processed_log_files::upsert(&mut tx, &pf_input("h1", "a.txt"))
            .await
            .unwrap();
        let ids = insert_batch_with_ledger(&mut tx, &[raw_input(pf_id, 0, "World1")])
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(ids.len(), 1);
        let raw_count: i64 = sqlx::query("SELECT COUNT(*) FROM raw_log_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        let ledger_count: i64 =
            sqlx::query("SELECT COUNT(*) FROM projected_raw_events WHERE raw_event_id = ?1")
                .bind(ids[0])
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(raw_count, 1);
        assert_eq!(ledger_count, 1);

        // ledger は Pending で start している
        let status: String =
            sqlx::query("SELECT status FROM projected_raw_events WHERE raw_event_id = ?1")
                .bind(ids[0])
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(status, "Pending");
    }

    /// 不変条件: 同 byte_offset を 2 回流しても重複しない。
    #[tokio::test]
    async fn duplicate_byte_offset_is_idempotent() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf_id = processed_log_files::upsert(&mut tx, &pf_input("h1", "a.txt"))
            .await
            .unwrap();
        let ids1 = insert_batch_with_ledger(&mut tx, &[raw_input(pf_id, 0, "World1")])
            .await
            .unwrap();
        let ids2 = insert_batch_with_ledger(&mut tx, &[raw_input(pf_id, 0, "World1-dupe")])
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(ids1, ids2, "duplicate must return the same raw id");
        let count: i64 = sqlx::query("SELECT COUNT(*) FROM raw_log_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 1);
        let ledger_count: i64 = sqlx::query("SELECT COUNT(*) FROM projected_raw_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(ledger_count, 1);
    }

    /// 不変条件: cursor 更新と raw insert を同一 tx に束ねるとアトミック。
    #[tokio::test]
    async fn cursor_update_in_same_tx_is_atomic() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf_id = processed_log_files::upsert(&mut tx, &pf_input("h1", "a.txt"))
            .await
            .unwrap();
        insert_batch_with_ledger(
            &mut tx,
            &[
                raw_input(pf_id, 0, "World1"),
                raw_input(pf_id, 100, "World2"),
            ],
        )
        .await
        .unwrap();
        set_ingest_position(&mut tx, pf_id, 200).await.unwrap();
        tx.commit().await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        let pos = get_ingest_position(&mut tx, pf_id).await.unwrap();
        let raw_count: i64 = sqlx::query("SELECT COUNT(*) FROM raw_log_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(pos, 200);
        assert_eq!(raw_count, 2);
    }

    /// 不変条件: tx を rollback すると raw も ledger も cursor も消える (all-or-nothing)。
    #[tokio::test]
    async fn rollback_undoes_raw_ledger_and_cursor_together() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf_id = processed_log_files::upsert(&mut tx, &pf_input("h1", "a.txt"))
            .await
            .unwrap();
        // pf 自体は別 tx でコミットしておく (rollback 対象でない)
        tx.commit().await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        insert_batch_with_ledger(&mut tx, &[raw_input(pf_id, 0, "World1")])
            .await
            .unwrap();
        set_ingest_position(&mut tx, pf_id, 100).await.unwrap();
        tx.rollback().await.unwrap();

        let raw_count: i64 = sqlx::query("SELECT COUNT(*) FROM raw_log_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        let ledger_count: i64 = sqlx::query("SELECT COUNT(*) FROM projected_raw_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        let mut tx = pool.begin().await.unwrap();
        let pos = get_ingest_position(&mut tx, pf_id).await.unwrap();
        assert_eq!(raw_count, 0);
        assert_eq!(ledger_count, 0);
        assert_eq!(pos, 0);
    }

    /// 起動時 recovery: orphan raw を検出できる。
    /// 通常運用では発生しないが、Phase 4a 以前のマイグレーション前段階や
    /// 異常系の自動補修に使う。
    #[tokio::test]
    async fn detects_orphan_raw_without_ledger() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf_id = processed_log_files::upsert(&mut tx, &pf_input("h1", "a.txt"))
            .await
            .unwrap();
        // 直接 raw だけ insert (ledger なし) — 故意に作る
        sqlx::query(
            "INSERT INTO raw_log_events (processed_log_file_id, byte_offset, event_type, payload_json)
             VALUES (?1, 0, 'RoomEntering', '{}')",
        )
        .bind(pf_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        let count = count_orphan_raw_without_ledger(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(count, 1);
    }

    /// orphan raw を ledger 補修 insert で 0 件にできる。
    #[tokio::test]
    async fn repair_orphan_ledger_inserts_pending() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf_id = processed_log_files::upsert(&mut tx, &pf_input("h1", "a.txt"))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO raw_log_events (processed_log_file_id, byte_offset, event_type, payload_json)
             VALUES (?1, 0, 'RoomEntering', '{}'), (?1, 1, 'RoomEntering', '{}')",
        )
        .bind(pf_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        let repaired = repair_orphan_ledger(&mut tx).await.unwrap();
        let remaining = count_orphan_raw_without_ledger(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(repaired, 2);
        assert_eq!(remaining, 0);

        // 補修後の ledger は Pending で 2 件
        let pending: i64 =
            sqlx::query("SELECT COUNT(*) FROM projected_raw_events WHERE status = 'Pending'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(pending, 2);
    }
}
