//! VRCWatchDog Tauri shell. Phase 5d スケルトン段階。
//!
//! 現状は [`run`] が Tauri を最低限起動するだけ。10-step bootstrap や IPC 配線、
//! frontend との結合は後続コミットで積む。
//!
//! `lib.rs` を分けてあるのは:
//! - Windows-only のリソース埋め込み (build.rs) を bin と分離するため
//! - 将来 mobile (`tauri::mobile_entry_point`) を追加するときに lib 側で再利用するため

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

pub mod paths;

/// Tauri アプリを起動する。
///
/// 失敗時は OS 例外コードと共に process exit するので戻り値は ()。
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("vrcwatchdog: phase 5d skeleton boot");

    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("tauri runtime failed to start");
}
