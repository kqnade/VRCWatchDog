//! プラン §9 の固定起動順序。
//!
//! 番号は plan §9 と一致。本 commit (Phase 5d) で wire できるのは `1, 2, 3, 4, 6, 8, 9, 10`。
//! 未実装 actor (`db_write_actor`, `thumb_writer`, `LogWatcher`, `PhotoScanner`,
//! `ProcessMonitor`, `Projector` 連続実行) は別 commit で `Bootstrap::new` 内に
//! 順次積む。
//!
//! ```text
//!  1. tracing 初期化               ← lib.rs::run() で先に実行
//!  2. tauri-plugin-single-instance ← lib.rs::run() で Builder に積む (最早)
//!  3. settings を read-only で load ← Bootstrap::new() でやる
//!  4. DB open + PRAGMA + migrate    ← Bootstrap::new() でやる
//!  5. db_write_actor 起動           ← TODO Phase 6+
//!  6. SettingsWriter / thumb_writer ← settings 部分のみ Bootstrap::new() でやる
//!  7. core actor 起動               ← TODO Phase 5e+
//!  8. autostart plugin              ← lib.rs::run() で Builder に積む
//!  9. window-state plugin           ← lib.rs::run() で Builder に積む
//! 10. メインウィンドウ作成 (--startup なら hidden) ← Phase 5e で setup() に追加
//! ```

use anyhow::{Context, Result};
use vrcwatchdog_core::db;
use vrcwatchdog_core::settings::{load_settings, LoadOutcome, SettingsWriter};

use crate::paths::AppPaths;
use crate::state::{AppState, SettingsCorruptInfo};

/// 起動時に揃える state 一式。`run()` から `tauri::async_runtime::block_on` で生成。
pub struct Bootstrap {
    pub state: AppState,
}

impl Bootstrap {
    /// step 3〜6 の同期構築。失敗時は anyhow context 付きで返す → 呼出側 (`run()`) は
    /// log 出力 + プロセス終了する想定 (UI を出す前に致命を露呈)。
    pub async fn new(paths: AppPaths) -> Result<Self> {
        // Step 4 の前に dir を保証する。settings.json と DB ファイル本体は触らない (plan §4)。
        paths
            .ensure_dirs()
            .context("could not create app data directories")?;

        // Step 3: settings を **read-only** で load。corrupt なら .corrupt-{ts}.bak が
        // 自動で作られる。ここでは write しない (plan §4 起動時 settings 自動 write 禁止)。
        let outcome = load_settings(&paths.settings_path)
            .with_context(|| format!("load_settings failed: {}", paths.settings_path.display()))?;
        let (initial_settings, settings_corrupt) = match outcome {
            LoadOutcome::Loaded(s) => (s, None),
            LoadOutcome::NotFound(s) => (s, None),
            LoadOutcome::CorruptBackedUp {
                settings,
                backup_path,
                reason,
            } => (
                settings,
                Some(SettingsCorruptInfo {
                    backup_path,
                    reason,
                }),
            ),
        };

        // Step 4: DB pool を WAL + foreign_keys + busy_timeout 付きで開き migrate。
        let db_pool = db::open(&paths.db_path)
            .await
            .with_context(|| format!("db::open failed: {}", paths.db_path.display()))?;

        // Step 6 (settings 部分のみ): writer actor を spawn。capacity=64 は plan §4 の推奨。
        // 起動時 write はしない (spawn 内で write しないことを SettingsWriter が保証)。
        let settings = SettingsWriter::spawn(paths.settings_path.clone(), initial_settings, 64);

        // OneDrive 同期 risk は detection だけしておき、emit は `setup()` で。
        let db_sync_risk = paths.detect_db_sync_risk();

        let state = AppState {
            paths,
            settings,
            db_pool,
            settings_corrupt,
            db_sync_risk,
        };

        Ok(Bootstrap { state })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use vrcwatchdog_core::settings::Settings;

    /// AppPaths を tempdir で組み立てる helper。
    fn temp_paths() -> (AppPaths, tempfile::TempDir, tempfile::TempDir) {
        let r = tempdir().unwrap();
        let l = tempdir().unwrap();
        let paths = AppPaths::with_bases(r.path().to_path_buf(), l.path().to_path_buf());
        (paths, r, l)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bootstrap_assembles_state_from_clean_directories() {
        let (paths, _r, _l) = temp_paths();
        let bootstrap = Bootstrap::new(paths.clone()).await.expect("bootstrap ok");

        // settings は無かったので default で立ち上がる
        assert_eq!(bootstrap.state.settings.snapshot(), Settings::default());
        assert!(bootstrap.state.settings_corrupt.is_none());

        // DB pool は使える
        let one: i64 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&bootstrap.state.db_pool)
            .await
            .expect("query");
        assert_eq!(one, 1);

        // OneDrive risk は無い (clean tempdir)
        assert!(bootstrap.state.db_sync_risk.is_none());

        // settings.json は **書かれていない** (plan §4 起動時自動 write 禁止)
        assert!(!paths.settings_path.exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bootstrap_propagates_corrupt_settings_backup_info() {
        let (paths, _r, _l) = temp_paths();
        // 親 dir だけ作って壊れた settings.json を仕込む
        fs::create_dir_all(&paths.roaming_dir).unwrap();
        fs::write(&paths.settings_path, b"{ broken json").unwrap();

        let bootstrap = Bootstrap::new(paths)
            .await
            .expect("bootstrap ok despite corrupt");

        // default で起動しつつ、corrupt info が AppState に持ち越される
        assert_eq!(bootstrap.state.settings.snapshot(), Settings::default());
        let info = bootstrap.state.settings_corrupt.expect("corrupt info");
        assert!(info.backup_path.exists());
        assert!(!info.reason.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bootstrap_detects_onedrive_db_path() {
        // local base に OneDrive を含めると detect_db_sync_risk が発火する
        let r = tempdir().unwrap();
        let l = tempdir().unwrap();
        // 実 OneDrive dir は作らないがパス検出はテキスト一致なので OK
        let l_path = l.path().join("OneDrive").join("Local");
        fs::create_dir_all(&l_path).unwrap();
        let paths = AppPaths::with_bases(r.path().to_path_buf(), l_path);

        let bootstrap = Bootstrap::new(paths).await.expect("bootstrap ok");
        let risk = bootstrap.state.db_sync_risk.expect("must detect onedrive");
        assert_eq!(risk.indicator, "OneDrive");
    }
}
