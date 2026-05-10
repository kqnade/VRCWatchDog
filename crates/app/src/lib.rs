//! VRCWatchDog Tauri shell。Phase 5d: 10-step bootstrap (1, 2, 3, 4, 6, 8, 9, 10) を wire。
//!
//! - `lib.rs` を分けてあるのは Windows-only の Resource 埋め込み (build.rs) を bin と
//!   分離するため、また将来 mobile (`tauri::mobile_entry_point`) を追加するときに
//!   lib 側で再利用するため。
//! - 残り step 5/7/10-tray は actor 実装と一緒に積む。

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

pub mod bootstrap;
pub mod commands;
pub mod paths;
pub mod state;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tauri::{Emitter, Manager};
use tokio::task::JoinSet;
use tracing_subscriber::EnvFilter;
use vrcwatchdog_core::ipc::events::{names, OneDriveWarning, SettingsCorruptWarning};
use vrcwatchdog_core::log_watcher::{
    LogWatcherActor, NotifyEventSource, RealFsProbe, WatcherConfig,
};
use vrcwatchdog_core::projector::project_batch;

use crate::bootstrap::Bootstrap;
use crate::paths::{default_vrchat_log_dir, AppPaths};
use crate::state::AppState;

/// projector batch の最大処理件数 (1 イテレーション)。
/// 値は plan §1 の backpressure 設計を踏まえつつ tail.rs と揃える。
const PROJECTOR_BATCH_SIZE: i64 = 500;

/// projector の poll 間隔。raw_log_events に push があってから projection されるまでの
/// 最大遅延の目安。
const PROJECTOR_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Tauri アプリを起動する。
///
/// 失敗時は OS 例外コードと共に process exit する。
pub fn run() {
    init_tracing();

    // Step 3-6 までを async で実行 (DB open + settings load + writer spawn)。
    // tauri::async_runtime は内部 tokio runtime を使う。block_on 終了後も spawn 済み
    // task は引き続き動く (settings_writer の actor loop 等)。
    let paths = match AppPaths::from_env() {
        Ok(p) => p,
        Err(e) => fatal("could not resolve app paths", e),
    };
    tracing::info!(
        ?paths.roaming_dir, ?paths.local_dir, ?paths.db_path,
        "vrcwatchdog: paths resolved"
    );

    let bootstrap = match tauri::async_runtime::block_on(Bootstrap::new(paths)) {
        Ok(b) => b,
        Err(e) => fatal("bootstrap failed", e),
    };

    // Step 7 (partial): log_watcher actor + projector loop を spawn。
    // JoinSet をローカルに保持し、Tauri Builder.run() が抜けた直後に drop すること
    // で全 task を abort する。state には積まないので、Bootstrap::new の同期テストは
    // この spawn の影響を受けない。
    let bg_tasks = tauri::async_runtime::block_on(spawn_background_tasks(&bootstrap.state));

    // Step 2: single-instance (最早登録)。2 番目に起動された process は plugin が
    // 起動済みインスタンスにシグナルを送り、自身は早期終了する。
    let single_instance = tauri_plugin_single_instance::init(|app, _args, _cwd| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.set_focus();
            let _ = window.unminimize();
        }
    });

    // Step 8: autostart toggle plugin。enable/disable は明示操作 (settings command)
    // からだけ呼ばせる (plan §4 起動時自動 write 禁止)。`--startup` 引数を autostart
    // 経由起動時に渡し、後段で window 初期可視性の判定に使う。
    let autostart = tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::LaunchAgent,
        Some(vec!["--startup"]),
    );

    // Step 9: window-state を保存/復元。
    let window_state = tauri_plugin_window_state::Builder::default().build();

    // Rust 側で `app.opener().open_path()` を使うために register。capability 側で
    // opener:* perm は付与しない (= JS から `plugin:opener|open_path` は呼べない)。
    let opener = tauri_plugin_opener::init();

    tauri::Builder::default()
        .plugin(single_instance)
        .plugin(window_state)
        .plugin(autostart)
        .plugin(opener)
        .manage(bootstrap.state)
        .invoke_handler(tauri::generate_handler![
            commands::open_photo,
            commands::open_photo_folder,
            commands::get_settings,
            commands::save_settings,
        ])
        .setup(|app| {
            // Bootstrap で検出した警告をここで emit する (event listener が貼られた
            // 直後に届くよう setup 内で投げる)。
            let state: tauri::State<'_, AppState> = app.state();

            if let Some(info) = &state.settings_corrupt {
                let payload = SettingsCorruptWarning {
                    backup_path: info.backup_path.clone(),
                    reason: info.reason.clone(),
                };
                if let Err(e) = app.emit(names::SETTINGS_CORRUPT_WARNING, payload) {
                    tracing::warn!(error = %e, "failed to emit settings corrupt warning");
                }
            }

            if let Some(risk) = &state.db_sync_risk {
                let payload = OneDriveWarning {
                    db_path: risk.db_path.clone(),
                    detected_indicator: risk.indicator.clone(),
                };
                if let Err(e) = app.emit(names::ONEDRIVE_WARNING, payload) {
                    tracing::warn!(error = %e, "failed to emit onedrive warning");
                }
            }

            // Step 10 (--startup の hidden 化) は Phase 5e で system tray 一緒に。
            // 現状は tauri.conf.json の visible:true をそのまま使う。
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri runtime failed to start");

    // Builder.run() から抜けたら (= app exit) 背景タスクを abort。
    // JoinSet::drop が abort_all を呼ぶので明示は不要だが、意図を明示するためにも書く。
    drop(bg_tasks);
}

/// log_watcher actor + projector loop を spawn して JoinSet として返す。
///
/// - log_watcher は `settings.log_directory` を最優先、未設定なら `default_vrchat_log_dir()`
///   を fallback として使う。どちらの path も `is_dir()` が false なら spawn しない (warn log)。
/// - projector loop は常時 spawn。raw_log_events が無くてもループは回り続け (no-op)、
///   後から log_watcher が事象を流し込んだ時点で processing が始まる。
async fn spawn_background_tasks(state: &AppState) -> JoinSet<()> {
    let mut tasks = JoinSet::new();

    // log_watcher: 有効な log_dir があれば spawn
    let configured = state.settings.snapshot().log_directory.clone();
    let effective_log_dir = configured.or_else(default_vrchat_log_dir);

    match effective_log_dir.as_deref() {
        Some(dir) if dir.is_dir() => {
            spawn_log_watcher(&mut tasks, state.db_pool.clone(), dir).await;
        }
        Some(dir) => {
            tracing::warn!(
                log_dir = %dir.display(),
                "log directory does not exist; log_watcher not started \
                 (set settings.log_directory or install VRChat to populate the default path)",
            );
        }
        None => {
            tracing::warn!(
                "no log directory configured and USERPROFILE missing; log_watcher not started",
            );
        }
    }

    // projector: 常時 spawn
    let pool = state.db_pool.clone();
    tasks.spawn(async move {
        loop {
            match project_batch(&pool, PROJECTOR_BATCH_SIZE).await {
                Ok(r) if r.processed > 0 => {
                    tracing::info!(
                        processed = r.processed,
                        done = r.done,
                        skipped = r.skipped,
                        failed = r.failed,
                        "projector batch",
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::error!(error = %e, "projector batch failed"),
            }
            tokio::time::sleep(PROJECTOR_POLL_INTERVAL).await;
        }
    });

    tasks
}

/// `spawn_log_watcher` 内部分け。エラー時は warn log だけで吸収し、
/// projector など他の background task の起動を阻害しない。
async fn spawn_log_watcher(tasks: &mut JoinSet<()>, pool: sqlx::SqlitePool, log_dir: &Path) {
    let probe = Arc::new(RealFsProbe::new());
    let source = match NotifyEventSource::new(log_dir) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                log_dir = %log_dir.display(),
                error = %e,
                "failed to initialize notify watcher; log_watcher not started",
            );
            return;
        }
    };
    let mut actor = LogWatcherActor::new(pool, source, probe, WatcherConfig::default());

    // 起動時 reconcile: 既存ファイルを catch-up。失敗時は warn だけして actor 起動は続行
    // (notify が以後の変更を捕捉する余地は残る)。
    let log_dir_for_log = log_dir.to_path_buf();
    if let Err(e) = actor.reconcile(log_dir).await {
        tracing::warn!(
            log_dir = %log_dir.display(),
            error = %e,
            "initial reconcile failed; log_watcher will still start",
        );
    } else {
        tracing::info!(log_dir = %log_dir.display(), "initial reconcile completed");
    }

    tasks.spawn(async move {
        if let Err(e) = actor.run().await {
            tracing::error!(
                log_dir = %log_dir_for_log.display(),
                error = %e,
                "log_watcher actor exited with error",
            );
        }
    });
    tracing::info!(log_dir = %log_dir.display(), "log_watcher spawned");
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init();
}

fn fatal(prefix: &str, e: impl std::fmt::Display) -> ! {
    tracing::error!(error = %e, "{prefix}");
    eprintln!("{prefix}: {e}");
    std::process::exit(1);
}
