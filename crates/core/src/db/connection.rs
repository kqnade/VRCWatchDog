use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

use crate::Result;

/// 共有 SQLite コネクションプール。
///
/// `read` は複数同時に走らせて良いが、`write` は [`super::write_actor`] 経由で
/// シリアル化する想定 (Phase 2 で雛形のみ、Phase 4a で本格利用)。
pub type Pool = SqlitePool;

/// SQLite DB を開いてマイグレーションまで実行する。
///
/// 設定 (PRAGMA):
/// - `journal_mode = WAL` — read/write 並列性のため
/// - `synchronous = NORMAL` — WAL の標準推奨値
/// - `busy_timeout = 5s` — 競合時の即時失敗を回避
/// - `foreign_keys = ON` — connection 単位で必須なので接続時に固定
pub async fn open(path: &Path) -> Result<Pool> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok(pool)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use sqlx::Row;
    use tempfile::tempdir;

    /// 1 件の processed_log_files + raw_log_events を作って raw event id を返す。
    async fn seed_raw_event(pool: &Pool) -> i64 {
        sqlx::query(
            "INSERT INTO processed_log_files (
                file_identity_hash, log_sequence_key, volume_serial,
                file_id_high, file_id_low, generation, creation_time, first_kb_hash,
                file_name, file_size, mtime,
                ingest_position, last_projected_raw_event_id,
                tz_id, tz_source, processed_at
            ) VALUES (
                'h1', '2026-05-09_00-00-00', 0, 0, 0, 0, 0, 'k1',
                'a.txt', 0, '2026-05-09T00:00:00Z',
                0, NULL, 'Asia/Tokyo', 'CapturedRealtime', '2026-05-09T00:00:00Z'
            )",
        )
        .execute(pool)
        .await
        .unwrap();
        let pf_id: i64 = sqlx::query("SELECT id FROM processed_log_files")
            .fetch_one(pool)
            .await
            .unwrap()
            .get(0);
        sqlx::query(
            "INSERT INTO raw_log_events (processed_log_file_id, byte_offset, event_type, payload_json)
             VALUES (?1, 0, 'RoomEntering', '{}')",
        )
        .bind(pf_id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("SELECT id FROM raw_log_events")
            .fetch_one(pool)
            .await
            .unwrap()
            .get(0)
    }

    /// PRAGMA が期待通り設定されていることを file-backed DB で確認する。
    #[tokio::test]
    async fn pragma_settings_are_applied() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let pool = open(&path).await.unwrap();

        let journal_mode: String = sqlx::query("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(journal_mode.to_lowercase(), "wal");

        let foreign_keys: i64 = sqlx::query("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(foreign_keys, 1);

        let synchronous: i64 = sqlx::query("PRAGMA synchronous")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        // synchronous = NORMAL は数値 1
        assert_eq!(synchronous, 1);

        let busy_timeout: i64 = sqlx::query("PRAGMA busy_timeout")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert!(busy_timeout >= 5000);
    }

    /// マイグレーション後に必須テーブルが存在することを確認する。
    #[tokio::test]
    async fn schema_has_expected_tables() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let pool = open(&path).await.unwrap();

        let expected = [
            "processed_log_files",
            "raw_log_events",
            "projected_raw_events",
            "world_visits",
            "player_sessions",
            "photo_records",
            "video_records",
            "notification_records",
            "failed_video_lookups",
        ];
        for name in expected {
            let row = sqlx::query("SELECT name FROM sqlite_master WHERE type='table' AND name=?1")
                .bind(name)
                .fetch_optional(&pool)
                .await
                .unwrap();
            assert!(row.is_some(), "table {name} not found");
        }
    }

    /// 同一 `source_raw_event_id` から world_visits 行を 2 件作れないことを確認する。
    /// 「同一 raw event ID から CREATE 系 domain 行は最大 1」不変条件のスキーマ保証。
    #[tokio::test]
    async fn unique_source_raw_event_id_blocks_duplicate_create() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let pool = open(&path).await.unwrap();

        let raw_id = seed_raw_event(&pool).await;

        // 1 回目: 成功
        sqlx::query(
            "INSERT INTO world_visits (
                source_raw_event_id, world_name,
                joined_naive_local, joined_utc,
                joined_tz_id, joined_offset_seconds,
                joined_resolution, joined_tz_source, joined_resolution_confidence
            ) VALUES (
                ?1, 'A',
                '2026-05-09 00:00:00', '2026-05-08T15:00:00Z',
                'Asia/Tokyo', 32400,
                'Single', 'CapturedRealtime', 'High'
            )",
        )
        .bind(raw_id)
        .execute(&pool)
        .await
        .unwrap();

        // 2 回目: UNIQUE 違反で失敗
        let res = sqlx::query(
            "INSERT INTO world_visits (
                source_raw_event_id, world_name,
                joined_naive_local, joined_utc,
                joined_tz_id, joined_offset_seconds,
                joined_resolution, joined_tz_source, joined_resolution_confidence
            ) VALUES (
                ?1, 'B',
                '2026-05-09 00:00:00', '2026-05-08T15:00:00Z',
                'Asia/Tokyo', 32400,
                'Single', 'CapturedRealtime', 'High'
            )",
        )
        .bind(raw_id)
        .execute(&pool)
        .await;
        assert!(
            res.is_err(),
            "duplicate source_raw_event_id must be rejected"
        );
    }

    /// projected_raw_events への 2 重 insert が PRIMARY KEY で拒否されることを確認する。
    /// 「ledger 1 raw event = 1 row」不変条件のスキーマ保証。
    #[tokio::test]
    async fn ledger_primary_key_blocks_duplicate() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let pool = open(&path).await.unwrap();

        let raw_id = seed_raw_event(&pool).await;

        sqlx::query(
            "INSERT INTO projected_raw_events (raw_event_id, status, created_at, updated_at)
             VALUES (?1, 'Pending', '2026-05-09T00:00:00Z', '2026-05-09T00:00:00Z')",
        )
        .bind(raw_id)
        .execute(&pool)
        .await
        .unwrap();

        let res = sqlx::query(
            "INSERT INTO projected_raw_events (raw_event_id, status, created_at, updated_at)
             VALUES (?1, 'Done', '2026-05-09T00:00:00Z', '2026-05-09T00:00:00Z')",
        )
        .bind(raw_id)
        .execute(&pool)
        .await;
        assert!(res.is_err(), "ledger duplicate must be rejected by PK");
    }

    /// raw_log_events から world_visits への ON DELETE RESTRICT が機能することを確認する。
    /// Phase 4 以降の idempotency 保証の前提となる不変条件。
    #[tokio::test]
    async fn raw_event_cannot_be_deleted_while_referenced_by_domain() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let pool = open(&path).await.unwrap();

        // 最小限の親行を作る: processed_log_files → raw_log_events → world_visits
        sqlx::query(
            "INSERT INTO processed_log_files (
                file_identity_hash, log_sequence_key, volume_serial,
                file_id_high, file_id_low, generation, creation_time, first_kb_hash,
                file_name, file_size, mtime,
                ingest_position, last_projected_raw_event_id,
                tz_id, tz_source, processed_at
            ) VALUES (
                'h1', '2026-05-09_00-00-00', 0, 0, 0, 0, 0, 'k1',
                'a.txt', 0, '2026-05-09T00:00:00Z',
                0, NULL, 'Asia/Tokyo', 'CapturedRealtime', '2026-05-09T00:00:00Z'
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let pf_id: i64 = sqlx::query("SELECT id FROM processed_log_files")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);

        sqlx::query(
            "INSERT INTO raw_log_events (processed_log_file_id, byte_offset, event_type, payload_json)
             VALUES (?1, 0, 'RoomEntering', '{}')",
        )
        .bind(pf_id)
        .execute(&pool)
        .await
        .unwrap();
        let raw_id: i64 = sqlx::query("SELECT id FROM raw_log_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);

        sqlx::query(
            "INSERT INTO world_visits (
                source_raw_event_id, world_name,
                joined_naive_local, joined_utc,
                joined_tz_id, joined_offset_seconds,
                joined_resolution, joined_tz_source, joined_resolution_confidence
            ) VALUES (
                ?1, 'TestWorld',
                '2026-05-09 00:00:00', '2026-05-08T15:00:00Z',
                'Asia/Tokyo', 32400,
                'Single', 'CapturedRealtime', 'High'
            )",
        )
        .bind(raw_id)
        .execute(&pool)
        .await
        .unwrap();

        // ON DELETE RESTRICT: raw 削除は失敗する
        let err = sqlx::query("DELETE FROM raw_log_events WHERE id=?1")
            .bind(raw_id)
            .execute(&pool)
            .await;
        assert!(err.is_err(), "raw delete must be rejected by FK RESTRICT");

        // processed_log_files 削除も raw が参照しているため拒否される
        let err = sqlx::query("DELETE FROM processed_log_files WHERE id=?1")
            .bind(pf_id)
            .execute(&pool)
            .await;
        assert!(
            err.is_err(),
            "processed_log_file delete must be rejected by FK RESTRICT"
        );
    }
}
