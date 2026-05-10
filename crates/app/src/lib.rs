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
use vrcwatchdog_core::ipc::events::names;
use vrcwatchdog_core::log_watcher::{
    LogWatcherActor, NotifyEventSource, RealFsProbe, WatcherConfig,
};
use vrcwatchdog_core::photo_scanner::{NotifyPhotoSource, PhotoScannerActor, PhotoScannerConfig};
use vrcwatchdog_core::projector::project_batch;
use vrcwatchdog_core::settings::Settings;
use vrcwatchdog_core::thumb_writer::{ThumbWriterActor, ThumbWriterConfig};

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
    // dialog: Settings 画面の folder picker。capability で `dialog:allow-open` のみ
    // 許可し、save dialog 等は使わせない (Phase 7.1.2 の用途は path 選択だけ)。
    let dialog = tauri_plugin_dialog::init();

    // Builder.build() を経由することで、setup() の外でも AppHandle / AppState に
    // アクセスできる。これにより background task spawn を build() 後に行えて、
    // health emitter 等で AppHandle が必要なケースに対応できる。
    let app = tauri::Builder::default()
        .plugin(single_instance)
        .plugin(window_state)
        .plugin(autostart)
        .plugin(opener)
        .plugin(dialog)
        .manage(bootstrap.state)
        .invoke_handler(tauri::generate_handler![
            commands::open_photo,
            commands::open_photo_folder,
            commands::get_settings,
            commands::save_settings,
            commands::get_initial_warnings,
            commands::list_recent_photos,
            commands::list_recent_visits,
            commands::list_recent_notifications,
        ])
        .setup(|_app| {
            // 起動時警告は `get_initial_warnings` command で frontend が pull する
            // 方式に置換済み (race 対策)。setup() で emit する旧方式は
            // listener attach 前のため取りこぼされていた。
            // Step 10 (--startup の hidden 化) は system tray と一緒に Phase 5e+ で。
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("tauri build failed");

    // Step 7: log_watcher / projector / health emitter を background で spawn。
    // build() 後だと AppHandle が取れるので health emitter に渡せる。
    // State 借用を run() 前に解放するため、scope で必要な値だけ owned で取り出す。
    let (handle, pool, settings_snapshot, db_path, thumb_dir) = {
        let state: tauri::State<'_, AppState> = app.state();
        (
            app.handle().clone(),
            state.db_pool.clone(),
            state.settings.snapshot(),
            state.paths.db_path.clone(),
            state.paths.thumb_dir.clone(),
        )
    };
    let bg_tasks = tauri::async_runtime::block_on(spawn_background_tasks(
        handle,
        pool,
        settings_snapshot,
        db_path,
        thumb_dir,
    ));

    // app.run() は self を消費して event loop 起動。Exit event で抜ける。
    // 抜けた後に bg_tasks を drop して JoinSet が abort_all を呼ぶ。
    app.run(|_handle, _event| {});
    drop(bg_tasks);
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
    thumb_dir: PathBuf,
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

    // photo_scanner: settings.photo_directory が設定済みかつ実在ディレクトリなら spawn。
    // log_watcher と違って fallback 先が無いので、未設定 / 不在は静かに skip
    // (UI に「写真フォルダを選んでください」プロンプトを出す前提)。
    if let Some(dir) = settings_snapshot.photo_directory.as_deref() {
        if dir.is_dir() {
            let tz = resolve_local_tz();
            spawn_photo_scanner(&mut tasks, pool.clone(), dir, tz).await;
        } else {
            tracing::warn!(
                photo_dir = %dir.display(),
                "photo_directory does not exist; photo_scanner not started",
            );
        }
    }

    // thumb_writer: photo_records から thumb_sha NULL の行を拾って webp を生成する。
    // photo_directory に依存しない (DB の中身だけ見る) ので無条件 spawn。
    // 既存写真がスキャン済みでも、thumb 未生成なら拾い続ける。
    spawn_thumb_writer(&mut tasks, pool.clone(), thumb_dir);

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

/// thumb_writer worker を spawn する。photo_directory 設定に依存せず常に走る。
/// photo_records の row は photo_scanner が入れるので、scanner が止まっていれば
/// thumb_writer も自然と空 batch loop になる (poll_interval 待機)。
fn spawn_thumb_writer(tasks: &mut JoinSet<()>, pool: SqlitePool, thumb_dir: PathBuf) {
    let config = ThumbWriterConfig {
        thumb_dir: thumb_dir.clone(),
        ..ThumbWriterConfig::default()
    };
    let actor = ThumbWriterActor::new(pool, config);
    tasks.spawn(async move {
        if let Err(e) = actor.run().await {
            tracing::error!(error = %e, "thumb_writer actor exited with error");
        }
    });
    tracing::info!(thumb_dir = %thumb_dir.display(), "thumb_writer spawned");
}

/// OS から local timezone を取得して `chrono_tz::Tz` に変換する。
///
/// 取得失敗 / parse 失敗時は UTC を返す。これは photo の `taken_naive_local` を
/// UTC に解決する際の前提として使う (DST 境界で Ambiguous/Gap を出す元)。
fn resolve_local_tz() -> chrono_tz::Tz {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| name.parse().ok())
        .unwrap_or(chrono_tz::UTC)
}

/// PhotoScannerActor を spawn する。NotifyPhotoSource の初期化失敗 / reconcile 失敗は
/// warn ログだけで吸収して他の background task を阻害しない (log_watcher と同じ方針)。
async fn spawn_photo_scanner(
    tasks: &mut JoinSet<()>,
    pool: SqlitePool,
    photo_dir: &Path,
    tz: chrono_tz::Tz,
) {
    let source = match NotifyPhotoSource::new(photo_dir) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                photo_dir = %photo_dir.display(),
                error = %e,
                "failed to initialize photo notify watcher; photo_scanner not started",
            );
            return;
        }
    };
    let mut actor = PhotoScannerActor::new(pool, source, PhotoScannerConfig { tz });

    // 起動時 reconcile: 既存写真を catch-up。失敗時は warn だけして actor 起動は続行。
    let dir_for_log = photo_dir.to_path_buf();
    match actor.reconcile(photo_dir).await {
        Ok(outcome) => tracing::info!(
            photo_dir = %photo_dir.display(),
            considered = outcome.considered,
            recorded = outcome.recorded,
            skipped = outcome.skipped,
            "photo_scanner initial reconcile completed",
        ),
        Err(e) => tracing::warn!(
            photo_dir = %photo_dir.display(),
            error = %e,
            "initial photo reconcile failed; photo_scanner will still start",
        ),
    }

    tasks.spawn(async move {
        if let Err(e) = actor.run().await {
            tracing::error!(
                photo_dir = %dir_for_log.display(),
                error = %e,
                "photo_scanner actor exited with error",
            );
        }
    });
    tracing::info!(photo_dir = %photo_dir.display(), "photo_scanner spawned");
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
