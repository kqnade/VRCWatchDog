//! `projected_raw_events` (ledger) の status 遷移。
//!
//! Phase 4a で `Pending` 行は raw insert と同一 tx で作られている。Phase 4b の
//! projector は raw を 1 件処理するごとに `Pending → Done | Skipped | FailedRecorded`
//! に進める。

use chrono::Utc;
use sqlx::{Sqlite, Transaction};

use crate::Result;

/// 1 raw を `Done` に進める。プロジェクション成功時に呼ぶ。
pub async fn mark_done(tx: &mut Transaction<'_, Sqlite>, raw_id: i64) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE projected_raw_events
         SET status = 'Done', updated_at = ?1, projected_at = ?1, error = NULL
         WHERE raw_event_id = ?2",
    )
    .bind(&now)
    .bind(raw_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 1 raw を `Skipped` に進める。projection 対象外 (UnparsableLine 等) のとき。
pub async fn mark_skipped(
    tx: &mut Transaction<'_, Sqlite>,
    raw_id: i64,
    reason: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE projected_raw_events
         SET status = 'Skipped', updated_at = ?1, projected_at = ?1, error = ?2
         WHERE raw_event_id = ?3",
    )
    .bind(&now)
    .bind(reason)
    .bind(raw_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 1 raw を `FailedRecorded` に進める。projection で例外を捕捉したが致命的でない場合。
pub async fn mark_failed(tx: &mut Transaction<'_, Sqlite>, raw_id: i64, error: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE projected_raw_events
         SET status = 'FailedRecorded', updated_at = ?1, projected_at = ?1, error = ?2
         WHERE raw_event_id = ?3",
    )
    .bind(&now)
    .bind(error)
    .bind(raw_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// projection 対象 (status = 'Pending') の raw を取り出す。
/// Codex v6 の指摘通り、 順序は `(log_sequence_key, generation, byte_offset)` で
/// reconcile が古いファイルを後から拾った場合でも時系列を保つ。
#[derive(Debug, Clone)]
pub struct PendingRaw {
    pub raw_event_id: i64,
    pub processed_log_file_id: i64,
    pub byte_offset: i64,
    pub event_type: String,
    pub payload_json: String,
    pub naive_local: Option<chrono::NaiveDateTime>,
    pub tz_id: String,
    pub tz_source: String,
}

type PendingRawRow = (
    i64,            // raw_event_id
    i64,            // processed_log_file_id
    i64,            // byte_offset
    String,         // event_type
    String,         // payload_json
    Option<String>, // naive_local
    String,         // tz_id
    String,         // tz_source
);

pub async fn fetch_pending_batch(
    tx: &mut Transaction<'_, Sqlite>,
    limit: i64,
) -> Result<Vec<PendingRaw>> {
    let rows: Vec<PendingRawRow> = sqlx::query_as(
        "SELECT r.id, r.processed_log_file_id, r.byte_offset, r.event_type,
                    r.payload_json, r.naive_local, p.tz_id, p.tz_source
             FROM projected_raw_events pr
             JOIN raw_log_events r ON r.id = pr.raw_event_id
             JOIN processed_log_files p ON p.id = r.processed_log_file_id
             WHERE pr.status = 'Pending'
             ORDER BY p.log_sequence_key ASC, p.generation ASC, r.byte_offset ASC
             LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(raw_event_id, pf_id, offset, event_type, payload, naive_str, tz_id, tz_source)| {
                let naive_local = naive_str.as_deref().and_then(|s| {
                    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()
                });
                PendingRaw {
                    raw_event_id,
                    processed_log_file_id: pf_id,
                    byte_offset: offset,
                    event_type,
                    payload_json: payload,
                    naive_local,
                    tz_id,
                    tz_source,
                }
            },
        )
        .collect())
}

/// 該当 file の `last_projected_raw_event_id` を更新する。 batch 完了後に呼ぶ。
pub async fn set_last_projected(
    tx: &mut Transaction<'_, Sqlite>,
    pf_id: i64,
    raw_event_id: i64,
) -> Result<()> {
    sqlx::query(
        "UPDATE processed_log_files
         SET last_projected_raw_event_id = ?1
         WHERE id = ?2 AND (last_projected_raw_event_id IS NULL OR last_projected_raw_event_id < ?1)",
    )
    .bind(raw_event_id)
    .bind(pf_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
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
    use sqlx::Row;
    use tempfile::tempdir;

    fn nd(h: u32) -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 5, 9)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap()
    }

    async fn setup() -> (sqlx::SqlitePool, tempfile::TempDir, i64, i64) {
        let dir = tempdir().unwrap();
        let pool = open(&dir.path().join("test.db")).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let pf_id = processed_log_files::upsert(
            &mut tx,
            &ProcessedLogFileInput {
                file_identity_hash: "h1".into(),
                log_sequence_key: "s".into(),
                volume_serial: 0,
                file_id_high: 0,
                file_id_low: 0,
                generation: 0,
                creation_time: 0,
                first_kb_hash: "k".into(),
                file_name: "a.txt".into(),
                file_size: 0,
                mtime: Utc::now(),
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
                byte_offset: 0,
                event: LogEvent::RoomEntering {
                    world_name: "Alpha".into(),
                },
                naive_local: Some(nd(20)),
            }],
        )
        .await
        .unwrap()[0];
        tx.commit().await.unwrap();
        (pool, dir, pf_id, raw_id)
    }

    #[tokio::test]
    async fn mark_done_advances_status() {
        let (pool, _dir, _pf_id, raw_id) = setup().await;
        let mut tx = pool.begin().await.unwrap();
        mark_done(&mut tx, raw_id).await.unwrap();
        let row: (String, Option<String>) = sqlx::query_as(
            "SELECT status, projected_at FROM projected_raw_events WHERE raw_event_id = ?1",
        )
        .bind(raw_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(row.0, "Done");
        assert!(row.1.is_some());
    }

    #[tokio::test]
    async fn fetch_pending_batch_orders_by_log_seq_then_offset() {
        let (pool, _dir, pf_id, _raw_id) = setup().await;
        // 既存 raw に加えて offset=100 と offset=50 を追加
        let mut tx = pool.begin().await.unwrap();
        raw_log::insert_batch_with_ledger(
            &mut tx,
            &[
                RawEventInput {
                    processed_log_file_id: pf_id,
                    byte_offset: 100,
                    event: LogEvent::RoomEntering {
                        world_name: "B".into(),
                    },
                    naive_local: Some(nd(22)),
                },
                RawEventInput {
                    processed_log_file_id: pf_id,
                    byte_offset: 50,
                    event: LogEvent::RoomEntering {
                        world_name: "AB".into(),
                    },
                    naive_local: Some(nd(21)),
                },
            ],
        )
        .await
        .unwrap();
        let pending = fetch_pending_batch(&mut tx, 10).await.unwrap();
        let offsets: Vec<i64> = pending.iter().map(|p| p.byte_offset).collect();
        assert_eq!(offsets, vec![0, 50, 100]);
    }

    #[tokio::test]
    async fn set_last_projected_only_advances() {
        let (pool, _dir, pf_id, raw_id) = setup().await;
        let mut tx = pool.begin().await.unwrap();
        set_last_projected(&mut tx, pf_id, raw_id).await.unwrap();
        // 同じ raw_id を再度入れても変わらない (= モノトニック)
        set_last_projected(&mut tx, pf_id, raw_id).await.unwrap();
        // raw_id - 1 を入れても巻き戻らない
        if raw_id > 1 {
            set_last_projected(&mut tx, pf_id, raw_id - 1)
                .await
                .unwrap();
        }
        let stored: i64 = sqlx::query(
            "SELECT last_projected_raw_event_id FROM processed_log_files WHERE id = ?1",
        )
        .bind(pf_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap()
        .get(0);
        assert_eq!(stored, raw_id);
    }
}
