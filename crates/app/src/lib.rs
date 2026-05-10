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
pub mod paths;
pub mod state;

use tauri::{Emitter, Manager};
use tracing_subscriber::EnvFilter;
use vrcwatchdog_core::ipc::events::{names, OneDriveWarning, SettingsCorruptWarning};

use crate::bootstrap::Bootstrap;
use crate::paths::AppPaths;
use crate::state::AppState;

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

    tauri::Builder::default()
        .plugin(single_instance)
        .plugin(window_state)
        .plugin(autostart)
        .manage(bootstrap.state)
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
