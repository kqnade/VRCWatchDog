//! `HealthStatus` event の組み立てと level 判定。
//!
//! [`collect_health`] が DB と fs から指標を 1 回 read して [`HealthStatus`] を返す。
//! [`classify`] は純関数で plan §1 の backpressure 閾値 (multi-axis) に従う。
//!
//! Phase 5e v0 では `projector_lag_sec` と `free_disk_bytes` を `0` に固定。
//! それぞれ別 commit で実装する:
//! - lag: `raw_log_events.naive_local` の最古 Pending との差分
//! - free disk: `sysinfo::Disks` で db_path の mount を引く
//!
//! `0` は「未計測」を意味するので、`classify` はその場合に Warning/Degraded を
//! 発火させない (誤陽性防止)。

use std::path::Path;

use sqlx::SqlitePool;

use crate::ipc::events::{HealthLevel, HealthStatus};
use crate::Result;

/// DB と fs から現状の健康状態を 1 回スナップショットする。
///
/// blocking I/O (`std::fs::metadata`) を呼ぶが db_path のサイズだけなので μs 程度。
pub async fn collect_health(pool: &SqlitePool, db_path: &Path) -> Result<HealthStatus> {
    let backlog_size: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM projected_raw_events WHERE status = 'Pending'")
            .fetch_one(pool)
            .await?;
    let backlog_size = backlog_size.max(0) as u64;

    let db_size_bytes = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);

    // TODO Phase 5f: raw_log_events.naive_local から最古 Pending との差分で計算
    let projector_lag_sec: i64 = 0;
    // TODO Phase 5f: sysinfo::Disks::new_with_refreshed_list() で db_path の mount を引く
    let free_disk_bytes: u64 = 0;

    let level = classify(
        backlog_size,
        projector_lag_sec,
        db_size_bytes,
        free_disk_bytes,
    );

    Ok(HealthStatus {
        backlog_size,
        projector_lag_sec,
        db_size_bytes,
        free_disk_bytes,
        level,
    })
}

/// plan §1 の backpressure 閾値 (multi-axis) に基づく level 判定。
///
/// | 指標                  | soft (Warning) | hard (Degraded)        |
/// |-----------------------|----------------|------------------------|
/// | backlog_size          | >= 30_000      | >= 50_000              |
/// | projector_lag_sec     | >= 60          | >= 300                 |
/// | db_size_bytes         | >= 1 GiB       | >= 5 GiB               |
/// | free_disk_bytes       | <= 5 GiB       | <= 1 GiB               |
///
/// 「いずれかの指標が hard を超過」→ Degraded。
/// 「いずれかが soft 以上 (hard 未満)」→ Warning。
/// それ以外は Healthy。
///
/// `free_disk_bytes == 0` は「未計測」を意味し、disk 系の判定をスキップする
/// (誤陽性防止)。`projector_lag_sec` は < 0 で同様にスキップ扱いになるが、
/// 計測値は常に >= 0 なので実質的に無効化されない。
pub fn classify(
    backlog_size: u64,
    projector_lag_sec: i64,
    db_size_bytes: u64,
    free_disk_bytes: u64,
) -> HealthLevel {
    const HARD_BACKLOG: u64 = 50_000;
    const HARD_LAG_SEC: i64 = 300;
    const HARD_DB_SIZE: u64 = 5 * 1024 * 1024 * 1024; // 5 GiB
    const HARD_FREE_DISK: u64 = 1024 * 1024 * 1024; // 1 GiB

    const SOFT_BACKLOG: u64 = 30_000;
    const SOFT_LAG_SEC: i64 = 60;
    const SOFT_DB_SIZE: u64 = 1024 * 1024 * 1024; // 1 GiB
    const SOFT_FREE_DISK: u64 = 5 * 1024 * 1024 * 1024; // 5 GiB

    // free_disk == 0 は「未計測」。disk 判定は計測されたときだけ有効化。
    let hard_disk_low = free_disk_bytes > 0 && free_disk_bytes <= HARD_FREE_DISK;
    let soft_disk_low = free_disk_bytes > 0 && free_disk_bytes <= SOFT_FREE_DISK;

    if backlog_size >= HARD_BACKLOG
        || projector_lag_sec >= HARD_LAG_SEC
        || db_size_bytes >= HARD_DB_SIZE
        || hard_disk_low
    {
        return HealthLevel::Degraded;
    }
    if backlog_size >= SOFT_BACKLOG
        || projector_lag_sec >= SOFT_LAG_SEC
        || db_size_bytes >= SOFT_DB_SIZE
        || soft_disk_low
    {
        return HealthLevel::Warning;
    }
    HealthLevel::Healthy
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use sqlx::Row;
    use tempfile::tempdir;

    // --- classify: 純関数の boundary table -----------------------------------------
    //
    // 1 軸ずつ閾値前後を確認する。複数軸の組合せを 1 テストでまとめると、どの軸の
    // 閾値が壊れても同じ test name が落ちて diagnosis が遅くなる。

    #[test]
    fn classify_returns_healthy_when_all_metrics_are_below_soft_thresholds() {
        // backlog/lag/db_size 全部 0、free_disk も 0 (= 未計測扱い)
        assert_eq!(classify(0, 0, 0, 0), HealthLevel::Healthy);
    }

    // backlog axis ----------------------------------------------------------------

    #[test]
    fn classify_returns_warning_when_backlog_reaches_soft_threshold() {
        assert_eq!(classify(30_000, 0, 0, 0), HealthLevel::Warning);
    }

    #[test]
    fn classify_returns_healthy_when_backlog_just_below_soft_threshold() {
        assert_eq!(classify(29_999, 0, 0, 0), HealthLevel::Healthy);
    }

    #[test]
    fn classify_returns_degraded_when_backlog_reaches_hard_threshold() {
        assert_eq!(classify(50_000, 0, 0, 0), HealthLevel::Degraded);
    }

    // projector_lag_sec axis ------------------------------------------------------

    #[test]
    fn classify_returns_warning_when_lag_reaches_soft_threshold() {
        assert_eq!(classify(0, 60, 0, 0), HealthLevel::Warning);
    }

    #[test]
    fn classify_returns_degraded_when_lag_reaches_hard_threshold() {
        assert_eq!(classify(0, 300, 0, 0), HealthLevel::Degraded);
    }

    // db_size axis ----------------------------------------------------------------

    #[test]
    fn classify_returns_warning_when_db_size_reaches_soft_threshold() {
        let one_gib: u64 = 1024 * 1024 * 1024;
        assert_eq!(classify(0, 0, one_gib, 0), HealthLevel::Warning);
    }

    #[test]
    fn classify_returns_degraded_when_db_size_reaches_hard_threshold() {
        let five_gib: u64 = 5 * 1024 * 1024 * 1024;
        assert_eq!(classify(0, 0, five_gib, 0), HealthLevel::Degraded);
    }

    // free_disk axis (lower-is-worse) ---------------------------------------------

    #[test]
    fn classify_treats_free_disk_zero_as_unknown_and_does_not_warn() {
        // 0 は「未計測」を意味する設計。disk 系の判定をスキップするので Healthy。
        assert_eq!(classify(0, 0, 0, 0), HealthLevel::Healthy);
    }

    #[test]
    fn classify_returns_warning_when_free_disk_drops_to_soft_threshold() {
        let five_gib: u64 = 5 * 1024 * 1024 * 1024;
        assert_eq!(classify(0, 0, 0, five_gib), HealthLevel::Warning);
    }

    #[test]
    fn classify_returns_degraded_when_free_disk_drops_to_hard_threshold() {
        let one_gib: u64 = 1024 * 1024 * 1024;
        assert_eq!(classify(0, 0, 0, one_gib), HealthLevel::Degraded);
    }

    #[test]
    fn classify_returns_healthy_when_free_disk_is_just_above_soft_threshold() {
        let above_soft: u64 = 5 * 1024 * 1024 * 1024 + 1;
        assert_eq!(classify(0, 0, 0, above_soft), HealthLevel::Healthy);
    }

    // 複数軸の競合: 「最も厳しい level が勝つ」 ----------------------------------

    #[test]
    fn classify_picks_degraded_when_any_axis_is_hard_even_if_others_are_healthy() {
        // backlog だけ degraded、他は健全
        assert_eq!(classify(50_000, 0, 0, 0), HealthLevel::Degraded);
    }

    // --- collect_health: file-backed SQLite で end-to-end ---------------------------
    //
    // backlog_size の SELECT COUNT が ledger 状態を反映することと、db_size_bytes が
    // metadata().len() で正しく取れることを確認する。Phase 5f の lag / free_disk は
    // 現状 0 placeholder なので別途 TODO test (後で追加)。

    /// テスト用に最小 schema を作る。本物の migrations を回すと core 全部が
    /// link されてテストが重くなるので、必要なテーブルだけ手書きで用意する。
    async fn open_minimal_pool(db_path: &Path) -> SqlitePool {
        let pool = crate::db::open(db_path).await.unwrap();
        pool
    }

    /// projected_raw_events に N 件の Pending を仕込んで返す。
    /// 親 raw_log_events / processed_log_files も連動して作る。
    async fn seed_pending(pool: &SqlitePool, count: usize) {
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
        .execute(pool)
        .await
        .unwrap();
        let pf_id: i64 = sqlx::query("SELECT id FROM processed_log_files")
            .fetch_one(pool)
            .await
            .unwrap()
            .get(0);

        for offset in 0..count {
            sqlx::query(
                "INSERT INTO raw_log_events (processed_log_file_id, byte_offset, event_type, payload_json)
                 VALUES (?1, ?2, 'RoomEntering', '{}')",
            )
            .bind(pf_id)
            .bind(offset as i64)
            .execute(pool)
            .await
            .unwrap();
            let raw_id: i64 = sqlx::query("SELECT id FROM raw_log_events ORDER BY id DESC LIMIT 1")
                .fetch_one(pool)
                .await
                .unwrap()
                .get(0);
            sqlx::query(
                "INSERT INTO projected_raw_events (raw_event_id, status, created_at, updated_at)
                 VALUES (?1, 'Pending', '2026-05-10T00:00:00Z', '2026-05-10T00:00:00Z')",
            )
            .bind(raw_id)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn collect_health_counts_pending_ledger_rows_as_backlog_size() {
        // Arrange
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("health.db");
        let pool = open_minimal_pool(&db_path).await;
        seed_pending(&pool, 17).await;

        // Act
        let h = collect_health(&pool, &db_path).await.unwrap();

        // Assert
        assert_eq!(
            h.backlog_size, 17,
            "Pending 行数が backlog_size に反映される"
        );
    }

    #[tokio::test]
    async fn collect_health_reports_zero_backlog_for_empty_ledger() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("health.db");
        let pool = open_minimal_pool(&db_path).await;

        let h = collect_health(&pool, &db_path).await.unwrap();

        assert_eq!(h.backlog_size, 0);
        assert_eq!(h.level, HealthLevel::Healthy);
    }

    #[tokio::test]
    async fn collect_health_reports_db_file_size_via_fs_metadata() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("health.db");
        let pool = open_minimal_pool(&db_path).await;
        // sqlite は init 直後でも > 0 byte (header 等)。具体値は env 依存なので
        // 「0 より大きい」以上は assert しない。
        seed_pending(&pool, 1).await;

        let h = collect_health(&pool, &db_path).await.unwrap();

        assert!(
            h.db_size_bytes > 0,
            "open + insert 後の sqlite ファイルは header 分以上のサイズを持つ"
        );
    }

    #[tokio::test]
    async fn collect_health_returns_zero_db_size_when_path_does_not_exist() {
        // Arrange: pool は別 path で作る。db_path 引数は意図的に存在しない path にする。
        let dir = tempdir().unwrap();
        let real_db = dir.path().join("real.db");
        let pool = open_minimal_pool(&real_db).await;
        let nonexistent = dir.path().join("nonexistent.db");

        // Act
        let h = collect_health(&pool, &nonexistent).await.unwrap();

        // Assert: metadata 失敗時は 0 にフォールバックして event 自体は出る
        assert_eq!(h.db_size_bytes, 0);
    }
}
