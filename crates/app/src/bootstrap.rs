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
    use tempfile::{tempdir, TempDir};
    use vrcwatchdog_core::settings::Settings;

    /// 各テストで `%APPDATA%` / `%LOCALAPPDATA%` を tempdir に隔離するための fixture。
    /// `TempDir` は drop で削除されるので、テスト関数末尾まで保持しておくのが呼び出し側の責務。
    struct IsolatedPaths {
        paths: AppPaths,
        _roaming_dir: TempDir,
        _local_dir: TempDir,
    }

    fn isolated_paths() -> IsolatedPaths {
        let roaming = tempdir().unwrap();
        let local = tempdir().unwrap();
        let paths = AppPaths::with_bases(roaming.path().to_path_buf(), local.path().to_path_buf());
        IsolatedPaths {
            paths,
            _roaming_dir: roaming,
            _local_dir: local,
        }
    }

    // -- 起動シナリオ #1: クリーンな空 dir からの起動 -------------------------------
    //
    // 「settings.json も DB も無い初回起動」が plan §4 の主要ハッピーパス。
    // この状態で Bootstrap が return したら以下が成立すべき:
    //   - in-memory snapshot が default (locale=ja 等)
    //   - settings_corrupt は None (corrupt は起きていない)
    //   - DB pool が動作する (SELECT 1 で round-trip)
    //   - db_sync_risk は None (tempdir はもちろん OneDrive 配下ではない)
    //   - settings.json は **書かれていない** (plan §4 起動時自動 write 禁止)
    //
    // 1 つの「クリーン起動」というシナリオに対する観察事項なので、
    // 上記 5 つを 1 テストにまとめる方が分かりやすい (シナリオ分割 vs assertion 分割)。

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clean_boot_yields_default_settings_working_pool_and_no_warnings() {
        // Arrange
        let env = isolated_paths();

        // Act
        let bootstrap = Bootstrap::new(env.paths.clone())
            .await
            .expect("bootstrap should succeed for clean dirs");

        // Assert: in-memory snapshot
        assert_eq!(bootstrap.state.settings.snapshot(), Settings::default());

        // Assert: warnings are absent
        assert!(bootstrap.state.settings_corrupt.is_none());
        assert!(bootstrap.state.db_sync_risk.is_none());

        // Assert: pool is usable end-to-end
        let one: i64 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&bootstrap.state.db_pool)
            .await
            .unwrap();
        assert_eq!(one, 1);

        // Assert: invariant — settings.json must NOT be written on a missing-file boot.
        assert!(
            !env.paths.settings_path.exists(),
            "plan §4: 起動時 settings 自動 write 禁止"
        );
    }

    // -- 起動シナリオ #2: corrupt settings からの起動 ------------------------------
    //
    // settings.json が壊れていた場合、`load_settings` 側が `.corrupt-{ts}.bak` を作って
    // default snapshot を返す (Phase 5b で実装済)。bootstrap はその結果を AppState に
    // 持ち越して、`setup()` 中で `SettingsCorruptWarning` を emit するソースにする。
    //
    // この commit でテストするのは bootstrap の責務:
    //   - default snapshot で立ち上がる
    //   - settings_corrupt に backup_path / reason が伝わる
    // backup ファイル自体の正しさは load_settings 側のテストで担保済み。

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn corrupt_settings_json_boots_with_defaults_and_carries_backup_info() {
        // Arrange: 壊れた JSON を仕込む
        let env = isolated_paths();
        fs::create_dir_all(&env.paths.roaming_dir).unwrap();
        fs::write(&env.paths.settings_path, b"{ broken json").unwrap();

        // Act
        let bootstrap = Bootstrap::new(env.paths.clone())
            .await
            .expect("bootstrap should still succeed when settings is corrupt");

        // Assert: default snapshot にフォールバック
        assert_eq!(bootstrap.state.settings.snapshot(), Settings::default());

        // Assert: corrupt info が `setup()` で emit できる形で残っている
        let info = bootstrap
            .state
            .settings_corrupt
            .expect("corrupt info should be propagated to AppState");
        assert!(
            info.backup_path.exists(),
            "load_settings は .corrupt-*.bak を実際に作っている"
        );
        assert!(
            !info.reason.is_empty(),
            "serde の parse error メッセージが reason に入る"
        );
    }

    // -- 起動シナリオ #3: DB が OneDrive 同期下にある ------------------------------
    //
    // db_path が OneDrive segment を含む場合、bootstrap は detect_db_sync_risk の結果を
    // AppState に持ち越し、`setup()` で OneDriveWarning を emit する。

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn onedrive_db_path_propagates_sync_risk_to_app_state() {
        // Arrange: local base に OneDrive segment を仕込んだ AppPaths
        let roaming = tempdir().unwrap();
        let local_root = tempdir().unwrap();
        let onedrive_local = local_root.path().join("OneDrive").join("Local");
        fs::create_dir_all(&onedrive_local).unwrap();
        let paths = AppPaths::with_bases(roaming.path().to_path_buf(), onedrive_local);

        // Act
        let bootstrap = Bootstrap::new(paths)
            .await
            .expect("bootstrap should succeed even when db is in OneDrive");

        // Assert
        let risk = bootstrap
            .state
            .db_sync_risk
            .expect("OneDrive segment must be detected as a sync risk");
        assert_eq!(risk.indicator, "OneDrive");
    }
}
