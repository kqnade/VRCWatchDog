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

use chrono::Local;
use sqlx::SqlitePool;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tokio::task::JoinSet;
use tracing_subscriber::EnvFilter;
use vrcwatchdog_core::db::repo::world_visits::{
    self, ExitFinalizeOutcome, TimeContext as VisitTimeContext,
};
use vrcwatchdog_core::health::collect_health;
use vrcwatchdog_core::ipc::events::names;
use vrcwatchdog_core::log_watcher::{
    LogWatcherActor, NotifyEventSource, RealFsProbe, WatcherConfig,
};
use vrcwatchdog_core::photo_scanner::{NotifyPhotoSource, PhotoScannerActor, PhotoScannerConfig};
use vrcwatchdog_core::process_monitor::{
    ProcessTransition, VRChatProcessMonitor, VRChatProcessMonitorConfig,
};
use vrcwatchdog_core::ipc::events::names as event_names;
use vrcwatchdog_core::projector::project_batch;
use vrcwatchdog_core::settings::Settings;
use vrcwatchdog_core::thumb_writer::{ThumbWriterActor, ThumbWriterConfig};
use vrcwatchdog_core::video_info::{VideoInfoActor, VideoInfoConfig};

use crate::bootstrap::Bootstrap;
use crate::paths::{default_vrchat_log_dir, AppPaths};
use crate::state::AppState;

/// projector batch の最大処理件数 (1 イテレーション)。
const PROJECTOR_BATCH_SIZE: i64 = 500;

/// main window を表示してフォーカスを当てる。tray の Show menu / 左クリックから呼ぶ。
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// main window が表示中なら hide、非表示なら show + focus。
fn toggle_main_window_visibility(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        match window.is_visible() {
            Ok(true) => {
                let _ = window.hide();
            }
            _ => show_main_window(app),
        }
    }
}

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
            commands::get_self_player,
            commands::list_recent_photos,
            commands::list_photos_for_visit,
            commands::list_recent_visits,
            commands::list_players_for_visit,
            commands::list_recent_notifications,
            commands::list_recent_videos,
        ])
        .setup(|app| {
            // Phase E: system tray + close-to-tray + --startup hidden launch。
            // 1. tray icon + Show/Quit menu を作成
            // 2. main window の close → hide に置換
            // 3. cli args に --startup が無ければウィンドウを show (visible:false が
            //    tauri.conf.json で初期値なので、通常起動時はここで明示 show する)
            let show_item = MenuItem::with_id(app, "show", "Show window", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let _tray = TrayIconBuilder::with_id("main")
                .tooltip("VRCWatchDog")
                .icon(app.default_window_icon().cloned().ok_or_else(|| {
                    tauri::Error::AssetNotFound("default window icon".into())
                })?)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_main_window_visibility(tray.app_handle());
                    }
                })
                .build(app)?;

            // close ボタンで quit せず hide-to-tray する
            if let Some(window) = app.get_webview_window("main") {
                let win_for_close = window.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = win_for_close.hide();
                    }
                });
            }

            // --startup 起動 (autostart 経由) ならそのまま hidden、それ以外は show。
            // settings の autostart_enabled が true でも、初回手動起動時は明示 show したい。
            let started_hidden = std::env::args().any(|a| a == "--startup");
            if !started_hidden {
                show_main_window(app.handle());
            }

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
    spawn_thumb_writer(&mut tasks, pool.clone(), thumb_dir.clone());

    // video_info actor (Phase D): video_records.title IS NULL の行を順次 noembed に
    // 問い合わせて metadata + thumbnail を補完する。失敗 URL は failed_video_lookups に
    // TTL 付きで negative cache。reqwest は network が無くても spawn 自体は失敗しないので
    // unconditional に起動 (offline なら transient_fail loop)。
    match VideoInfoActor::new(pool.clone(), VideoInfoConfig::new(thumb_dir)) {
        Ok(actor) => {
            tasks.spawn(async move { actor.run().await });
            tracing::info!("video_info actor spawned");
        }
        Err(e) => {
            tracing::warn!(error = %e, "video_info actor failed to start; metadata fetch disabled");
        }
    }

    // process_monitor + finalize coordinator (Phase 7.4.2):
    // monitor は 2s 間隔で polling、Started/Stopped を mpsc で送出。
    // coordinator は Stopped を受信して world_visits を finalize する。
    // capacity 8: Stopped/Started が高頻度で来ることはない (= 起動/終了時に 1 件)。
    let (proc_tx, mut proc_rx) = tokio::sync::mpsc::channel::<ProcessTransition>(8);
    let monitor =
        VRChatProcessMonitor::new(VRChatProcessMonitorConfig::default()).with_sender(proc_tx);
    tasks.spawn(async move { monitor.run().await });

    let pool_for_finalize = pool.clone();
    let tz_for_finalize = resolve_local_tz();
    tasks.spawn(async move {
        while let Some(transition) = proc_rx.recv().await {
            if !matches!(transition, ProcessTransition::Stopped) {
                continue;
            }
            // VRChat exit を観測したので、active な world_visit を 1 件 finalize。
            // plan §2 「log catch-up 完了後の finalize」 を厳密に守るには projector の
            // backlog drain を待つべきだが、現状は projector が常に poll loop で drain
            // しているので、process exit から数秒で catch-up が完了する想定。
            // ここでは即 finalize でも実害は小さい (Joining が遅延して active 中なら
            // ClosedWithoutJoin 化、Resolved なら left_utc stamp のみ)。
            if let Err(e) = finalize_active_visit_on_exit(&pool_for_finalize, tz_for_finalize).await
            {
                tracing::warn!(error = %e, "process_exit finalize failed");
            }
        }
    });
    tracing::info!("process_monitor + finalize coordinator spawned");

    // projector: 常時 spawn。raw が無くても loop は回り続け (no-op)、
    // 後から log_watcher が事象を流し込んだ時点で processing が始まる。
    // Phase B: Done になった raw event を LiveLogEvent として frontend に emit。
    let pool_proj = pool.clone();
    let app_for_proj = app.clone();
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
                    // commit 後 (= raw 永続化済み) なので emit 順は任意で OK。
                    // frontend が listener attach 前なら取りこぼすが、起動時の catch-up
                    // 分は /history などで読み返せるので realtime 視点では問題なし。
                    use tauri::Emitter;
                    for ev in r.events {
                        if let Err(e) = app_for_proj.emit(event_names::LIVE_LOG_EVENT, &ev) {
                            tracing::warn!(error = %e, "live_log_event emit failed");
                        }
                    }
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

/// VRChat process exit を受けて active world_visit を finalize する coordinator helper。
///
/// 1 transaction で `world_visits::finalize_active_on_process_exit` を実行し、
/// 結果を log に落とす。Pending → ClosedWithoutJoin / Resolved → left_utc stamp の
/// 振り分けは repo 側が判断。
async fn finalize_active_visit_on_exit(
    pool: &SqlitePool,
    tz: chrono_tz::Tz,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let now_local = Local::now().naive_local();
    let ctx = VisitTimeContext {
        tz_id: tz.name(),
        tz_source: "CapturedRealtime",
        resolution_confidence: "High",
    };
    let mut tx = pool.begin().await?;
    let outcome = world_visits::finalize_active_on_process_exit(&mut tx, now_local, ctx, None)
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    tx.commit().await?;
    match outcome {
        ExitFinalizeOutcome::NoActive => {
            tracing::info!("process_exit: no active visit to finalize");
        }
        ExitFinalizeOutcome::ClosedPendingAsWithoutJoin { visit_id } => {
            tracing::info!(visit_id, "process_exit: Pending → ClosedWithoutJoin");
        }
        ExitFinalizeOutcome::StampedLeftUtcOnResolved { visit_id } => {
            tracing::info!(visit_id, "process_exit: Resolved left_utc stamped");
        }
    }
    Ok(())
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
