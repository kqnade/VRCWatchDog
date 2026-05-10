//! `HealthStatus` event の組み立てと level 判定。
//!
//! [`collect_health`] が DB と fs から指標を 1 回 read して [`HealthStatus`] を返す。
//! [`classify`] は純関数で plan §1 の backpressure 閾値 (multi-axis) に従う。
//!
//! Phase 5f で `projector_lag_sec` と `free_disk_bytes` を実装した。
//! `free_disk_bytes` は `sysinfo::Disks` で db_path の mount を引いて算出。
//! `projector_lag_sec` は最古の Pending raw event の `naive_local` と現在のローカル時刻の
//! 差分 (秒)。両方とも naive_local は file の OS ローカル tz と仮定 (現状 99% のケース)。
//! tz が混在するケースは plan §6 の resolution と組み合わせる別 commit で改善する。
//!
//! `free_disk_bytes == 0` / `projector_lag_sec == 0` は「未計測 or 該当なし」を意味する
//! ため、`classify` はそれらを誤陽性扱いしない (= Warning/Degraded を発火させない)。

use std::path::Path;

use chrono::{Local, NaiveDateTime};
use sqlx::SqlitePool;
use sysinfo::Disks;

use crate::ipc::events::{HealthLevel, HealthStatus};
use crate::Result;

/// `naive_local` カラムのフォーマット (`raw_log.rs::insert_batch_with_ledger` と一致)。
const NAIVE_LOCAL_FMT: &str = "%Y-%m-%d %H:%M:%S";

/// DB と fs から現状の健康状態を 1 回スナップショットする。
///
/// blocking I/O (`std::fs::metadata` / `sysinfo::Disks`) を含むが、それぞれ ms 未満。
pub async fn collect_health(pool: &SqlitePool, db_path: &Path) -> Result<HealthStatus> {
    let backlog_size: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM projected_raw_events WHERE status = 'Pending'")
            .fetch_one(pool)
            .await?;
    let backlog_size = backlog_size.max(0) as u64;

    let db_size_bytes = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    let projector_lag_sec = compute_projector_lag_sec(pool, Local::now().naive_local()).await?;
    let free_disk_bytes = compute_free_disk_bytes(db_path);

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

/// 最古の Pending raw event の `naive_local` から `now_local` までの秒数。
///
/// - Pending が無い / 全 Pending の `naive_local` が NULL → 0
/// - 解析不能な `naive_local` 文字列 → 0 (skip)
/// - `now_local < oldest_pending_local` (時計巻き戻し / log の先取り) → 0 (max(0))
///
/// `now_local` を引数で受けて純関数化することでテスト可能性を保つ。
pub async fn compute_projector_lag_sec(pool: &SqlitePool, now_local: NaiveDateTime) -> Result<i64> {
    let oldest: Option<String> = sqlx::query_scalar(
        "SELECT MIN(rle.naive_local)
         FROM projected_raw_events p
         JOIN raw_log_events rle ON rle.id = p.raw_event_id
         WHERE p.status = 'Pending' AND rle.naive_local IS NOT NULL",
    )
    .fetch_optional(pool)
    .await?
    .flatten();

    let Some(s) = oldest else {
        return Ok(0);
    };
    let Ok(oldest_local) = NaiveDateTime::parse_from_str(&s, NAIVE_LOCAL_FMT) else {
        // フォーマット不一致は parser バグ等を示すが、metric のために panic する価値は無い。
        // 0 にフォールバックして tracing::warn で記録する。
        tracing::warn!(value = %s, "could not parse naive_local for projector lag");
        return Ok(0);
    };

    Ok((now_local - oldest_local).num_seconds().max(0))
}

/// `db_path` を含む mount point の available_space を返す。見つからなければ 0 (= 未計測)。
///
/// 複数の mount が prefix にマッチする場合は **最長 prefix が勝つ** (nested mount 対応)。
pub fn compute_free_disk_bytes(db_path: &Path) -> u64 {
    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<&sysinfo::Disk> = None;
    for d in &disks {
        if !db_path.starts_with(d.mount_point()) {
            continue;
        }
        let pick = match best {
            None => true,
            Some(prev) => d.mount_point().as_os_str().len() > prev.mount_point().as_os_str().len(),
        };
        if pick {
            best = Some(d);
        }
    }
    best.map(|d| d.available_space()).unwrap_or(0)
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

    /// 現状最低限の migrations を持つ DB pool を 1 行で作るための便宜 helper。
    async fn open_pool(db_path: &Path) -> SqlitePool {
        crate::db::open(db_path).await.unwrap()
    }

    /// processed_log_files を 1 行作って id を返す。各テストの最小親行。
    async fn seed_processed_log_file(pool: &SqlitePool) -> i64 {
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
        sqlx::query("SELECT id FROM processed_log_files")
            .fetch_one(pool)
            .await
            .unwrap()
            .get(0)
    }

    /// 1 件の raw_log_events を Pending ledger 付きで挿入して raw_id を返す。
    /// `naive_local` は呼び出し側が指定 (None なら NULL = lag に効かない)。
    async fn seed_pending_event(
        pool: &SqlitePool,
        pf_id: i64,
        byte_offset: i64,
        naive_local: Option<&str>,
    ) -> i64 {
        sqlx::query(
            "INSERT INTO raw_log_events
                (processed_log_file_id, byte_offset, event_type, payload_json, naive_local)
             VALUES (?1, ?2, 'RoomEntering', '{}', ?3)",
        )
        .bind(pf_id)
        .bind(byte_offset)
        .bind(naive_local)
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
        raw_id
    }

    /// `seed_pending_event` を N 回回す。lag が関係しないテストでは naive_local=None で
    /// 呼ぶと collect_health の lag 計算は 0 のまま。
    async fn seed_n_pending_without_naive_local(pool: &SqlitePool, count: usize) {
        let pf_id = seed_processed_log_file(pool).await;
        for offset in 0..count {
            seed_pending_event(pool, pf_id, offset as i64, None).await;
        }
    }

    // backlog_size 軸 ----------------------------------------------------------

    #[tokio::test]
    async fn collect_health_counts_pending_ledger_rows_as_backlog_size() {
        // Arrange
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("health.db");
        let pool = open_pool(&db_path).await;
        seed_n_pending_without_naive_local(&pool, 17).await;

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
        let pool = open_pool(&db_path).await;

        let h = collect_health(&pool, &db_path).await.unwrap();

        assert_eq!(h.backlog_size, 0);
        // level は free_disk を計測するようになったため dev 機の空き容量に依存。
        // ここでは backlog_size 0 を保証することが本テストの責務。level は別軸テストに分離。
    }

    // db_size_bytes 軸 ---------------------------------------------------------

    #[tokio::test]
    async fn collect_health_reports_db_file_size_via_fs_metadata() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("health.db");
        let pool = open_pool(&db_path).await;
        seed_n_pending_without_naive_local(&pool, 1).await;

        let h = collect_health(&pool, &db_path).await.unwrap();

        assert!(
            h.db_size_bytes > 0,
            "open + insert 後の sqlite ファイルは header 分以上のサイズを持つ"
        );
    }

    #[tokio::test]
    async fn collect_health_returns_zero_db_size_when_path_does_not_exist() {
        let dir = tempdir().unwrap();
        let real_db = dir.path().join("real.db");
        let pool = open_pool(&real_db).await;
        let nonexistent = dir.path().join("nonexistent.db");

        let h = collect_health(&pool, &nonexistent).await.unwrap();

        assert_eq!(
            h.db_size_bytes, 0,
            "metadata 失敗時は 0 にフォールバックして event 自体は出る"
        );
    }

    // --- compute_projector_lag_sec: pure-time-injected ----------------------------
    //
    // 純関数化するため `now_local` を引数で渡す設計にしてある。これにより
    // 「実時間に依存しない deterministic な assertion」が書ける。

    fn nd(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[tokio::test]
    async fn projector_lag_returns_zero_when_no_pending_events_exist() {
        let dir = tempdir().unwrap();
        let pool = open_pool(&dir.path().join("lag.db")).await;

        let lag = compute_projector_lag_sec(&pool, nd("2026-05-10 12:00:00"))
            .await
            .unwrap();

        assert_eq!(lag, 0);
    }

    #[tokio::test]
    async fn projector_lag_returns_zero_when_all_pending_naive_local_are_null() {
        // Arrange: Pending 行はあるが naive_local は NULL (例: UnparsableLine)
        let dir = tempdir().unwrap();
        let pool = open_pool(&dir.path().join("lag.db")).await;
        let pf_id = seed_processed_log_file(&pool).await;
        seed_pending_event(&pool, pf_id, 0, None).await;

        // Act
        let lag = compute_projector_lag_sec(&pool, nd("2026-05-10 12:00:00"))
            .await
            .unwrap();

        // Assert
        assert_eq!(lag, 0, "NULL naive_local は lag 計算から除外される");
    }

    #[tokio::test]
    async fn projector_lag_returns_seconds_between_oldest_pending_and_now_local() {
        // Arrange: Pending 2 件、最古は 12:00:00、新しい方は 12:01:00
        let dir = tempdir().unwrap();
        let pool = open_pool(&dir.path().join("lag.db")).await;
        let pf_id = seed_processed_log_file(&pool).await;
        seed_pending_event(&pool, pf_id, 0, Some("2026-05-10 12:00:00")).await;
        seed_pending_event(&pool, pf_id, 1, Some("2026-05-10 12:01:00")).await;

        // Act: now = 12:05:30 → 5min 30sec = 330sec
        let lag = compute_projector_lag_sec(&pool, nd("2026-05-10 12:05:30"))
            .await
            .unwrap();

        // Assert: 最古 (12:00:00) との diff
        assert_eq!(lag, 330);
    }

    #[tokio::test]
    async fn projector_lag_returns_zero_when_now_is_earlier_than_oldest_pending() {
        // 時計巻き戻し / log 先取りシナリオ。負値にならず 0 で saturate する。
        let dir = tempdir().unwrap();
        let pool = open_pool(&dir.path().join("lag.db")).await;
        let pf_id = seed_processed_log_file(&pool).await;
        seed_pending_event(&pool, pf_id, 0, Some("2026-05-10 12:00:00")).await;

        let lag = compute_projector_lag_sec(&pool, nd("2026-05-10 11:00:00"))
            .await
            .unwrap();

        assert_eq!(lag, 0, "now < oldest_pending は max(0) で 0 に saturate");
    }

    #[tokio::test]
    async fn projector_lag_falls_back_to_zero_when_naive_local_is_unparseable() {
        // データ整合性違反 (parser バグ等) の防衛: panic せず 0 を返し、tracing::warn 経由で
        // 通報するだけ。metric が 1 サンプル誤るより app が落ちる方が悪い。
        let dir = tempdir().unwrap();
        let pool = open_pool(&dir.path().join("lag.db")).await;
        let pf_id = seed_processed_log_file(&pool).await;
        seed_pending_event(&pool, pf_id, 0, Some("not-a-timestamp")).await;

        let lag = compute_projector_lag_sec(&pool, nd("2026-05-10 12:00:00"))
            .await
            .unwrap();

        assert_eq!(lag, 0);
    }

    // --- compute_free_disk_bytes: sysinfo ベースなので「正の値が返る」のみ assert -----
    //
    // mock 不能な real-fs sysinfo 呼び出しなので具体値は assert できない。
    // db_path に標準的な mounted disk 上の path (tempdir) を渡せば、必ずどれかの mount に
    // ヒットして available_space() > 0 になる前提でテストする。

    #[test]
    fn free_disk_returns_positive_value_for_path_on_mounted_disk() {
        // Arrange: tempdir() は OS の標準 temp dir 配下 = 必ず mounted disk 上
        let dir = tempdir().unwrap();

        // Act
        let free = compute_free_disk_bytes(dir.path());

        // Assert
        assert!(
            free > 0,
            "標準 temp dir は必ずどこかの mount に属し、空き容量は正のはず: got {free}"
        );
    }
}
