//! VRCWatchDog Tauri shell。Phase 5e: 10-step bootstrap + log_watcher + projector +
//! health-status emitter まで wire 済。
//!
//! `lib.rs` を分けてあるのは Windows-only の Resource 埋め込み (build.rs) を bin と
//! 分離するため、また将来 mobile (`tauri::mobile_entry_point`) を追加するときに lib
//! 側で再利用するため。

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

pub mod bootstrap;
pub mod commands;
pub mod paths;
pub mod state;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager};
use tokio::task::JoinSet;
use tracing_subscriber::EnvFilter;
use vrcwatchdog_core::health::collect_health;
use vrcwatchdog_core::ipc::events::{names, OneDriveWarning, SettingsCorruptWarning};
use vrcwatchdog_core::log_watcher::{
    LogWatcherActor, NotifyEventSource, RealFsProbe, WatcherConfig,
};
use vrcwatchdog_core::projector::project_batch;
use vrcwatchdog_core::settings::Settings;

use crate::bootstrap::Bootstrap;
use crate::paths::{default_vrchat_log_dir, AppPaths};
use crate::state::AppState;

/// projector batch の最大処理件数 (1 イテレーション)。
const PROJECTOR_BATCH_SIZE: i64 = 500;

/// projector の poll 間隔。raw_log_events に push があってから projection されるまでの
/// 最大遅延の目安。
const PROJECTOR_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// health-status event の emit 間隔。plan §1 では「短い間隔 (1〜5 秒) で frontend に
/// push する想定」。
const HEALTH_EMIT_INTERVAL: Duration = Duration::from_secs(2);

/// Tauri アプリを起動する。
///
/// 失敗時は OS 例外コードと共に process exit する。
pub fn run() {
    init_tracing();

    // Step 3-6: settings load + DB open + SettingsWriter spawn (sync portion of bootstrap)
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

    // Plugins
    let single_instance = tauri_plugin_single_instance::init(|app, _args, _cwd| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.set_focus();
            let _ = window.unminimize();
        }
    });
    let autostart = tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::LaunchAgent,
        Some(vec!["--startup"]),
    );
    let window_state = tauri_plugin_window_state::Builder::default().build();
    // Rust 側 `app.opener().open_path()` 専用。capability では opener:* perm を付けない。
    let opener = tauri_plugin_opener::init();

    // Builder.build() を経由することで、setup() の外でも AppHandle / AppState に
    // アクセスできる。これにより background task spawn を build() 後に行えて、
    // health emitter 等で AppHandle が必要なケースに対応できる。
    let app = tauri::Builder::default()
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
            emit_deferred_warnings(app);
            // Step 10 (--startup の hidden 化) は system tray と一緒に Phase 5e+ で。
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("tauri build failed");

    // Step 7: log_watcher / projector / health emitter を background で spawn。
    // build() 後だと AppHandle が取れるので health emitter に渡せる。
    // State 借用を run() 前に解放するため、scope で必要な値だけ owned で取り出す。
    let (handle, pool, settings_snapshot, db_path) = {
        let state: tauri::State<'_, AppState> = app.state();
        (
            app.handle().clone(),
            state.db_pool.clone(),
            state.settings.snapshot(),
            state.paths.db_path.clone(),
        )
    };
    let bg_tasks = tauri::async_runtime::block_on(spawn_background_tasks(
        handle,
        pool,
        settings_snapshot,
        db_path,
    ));

    // app.run() は self を消費して event loop 起動。Exit event で抜ける。
    // 抜けた後に bg_tasks を drop して JoinSet が abort_all を呼ぶ。
    app.run(|_handle, _event| {});
    drop(bg_tasks);
}

/// `Bootstrap` で検出済みの起動時警告を emit する (best-effort)。
///
/// frontend 側 listener が onMount 後に attach される一方、本関数は setup()
/// (= webview load 前) で呼ばれるため、初回 emit は取りこぼされる可能性がある。
/// Phase 5e 後続で「初期警告を再取得する command」を追加して取り回しを改善する。
fn emit_deferred_warnings(app: &tauri::App) {
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
}

/// log_watcher actor + projector loop + health emitter を spawn。
///
/// 戻り値の `JoinSet` を呼び出し側がローカルで持ち、`app.run()` 完了後に drop する
/// ことで全 task を abort する。
async fn spawn_background_tasks(
    app: AppHandle,
    pool: SqlitePool,
    settings_snapshot: Settings,
    db_path: PathBuf,
) -> JoinSet<()> {
    let mut tasks = JoinSet::new();

    // log_watcher: 有効な log_dir があれば spawn。settings の値が最優先、
    // 未設定なら VRChat の標準パスに fallback。どちらも is_dir() が false なら skip。
    let effective_log_dir = settings_snapshot
        .log_directory
        .clone()
        .or_else(default_vrchat_log_dir);
    match effective_log_dir.as_deref() {
        Some(dir) if dir.is_dir() => {
            spawn_log_watcher(&mut tasks, pool.clone(), dir).await;
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

    // projector: 常時 spawn。raw が無くても loop は回り続け (no-op)、
    // 後から log_watcher が事象を流し込んだ時点で processing が始まる。
    let pool_proj = pool.clone();
    tasks.spawn(async move {
        loop {
            match project_batch(&pool_proj, PROJECTOR_BATCH_SIZE).await {
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

    // health emitter: 2 秒ごとに collect_health → emit。
    // tokio::time::interval は first tick が即返るので、起動直後に 1 発投げて
    // frontend の "loading..." 表示を素早く解消する。
    let pool_health = pool;
    let handle_health = app;
    let db_path_health = db_path;
    tasks.spawn(async move {
        let mut ticker = tokio::time::interval(HEALTH_EMIT_INTERVAL);
        loop {
            ticker.tick().await;
            match collect_health(&pool_health, &db_path_health).await {
                Ok(payload) => {
                    if let Err(e) = handle_health.emit(names::HEALTH_STATUS, payload) {
                        tracing::warn!(error = %e, "failed to emit health status");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "failed to collect health"),
            }
        }
    });

    tasks
}

/// `spawn_log_watcher` 内部分け。エラー時は warn log だけで吸収し、
/// projector など他の background task の起動を阻害しない。
async fn spawn_log_watcher(tasks: &mut JoinSet<()>, pool: SqlitePool, log_dir: &Path) {
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
