//! `#[tauri::command]` wrappers around `vrcwatchdog_core::ipc::commands`.
//!
//! 本モジュールは frontend の `invoke()` から到達する唯一の接点。設計方針 (plan §7、
//! Tauri 2.4 で shell→opener に分離されたので opener で読み替える):
//!
//! 1. **opener の JS perm は capability で外す** → JS から `plugin:opener|open_*` は呼べない。
//! 2. 写真原本は asset:// で公開しない。
//! 3. 原本を OS シェルで開きたい時は、ここの `open_photo` Rust command を経由するしかない。
//! 4. `open_photo` は core::ipc::commands::handle_open_photo に validation (extension /
//!    canonicalize / scope) を委譲し、validated path のみを `app.opener().open_path()` に渡す。
//!
//! こうすることで「JS が `../../etc/passwd` を invoke しても、Rust 側のスコープ check で
//! 弾かれて opener に到達しない」という path traversal の hard guard を確保する。

use std::path::PathBuf;

use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use vrcwatchdog_core::ipc::commands as core_cmd;
use vrcwatchdog_core::settings::Settings;

use crate::state::AppState;

/// 写真原本を OS の関連付けされたアプリケーションで開く。
///
/// 戻り値は `Result<(), String>` (Tauri command 慣例: error は serialize 可能な型)。
#[tauri::command]
pub async fn open_photo(
    file_path: PathBuf,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let settings = state.settings.snapshot();
    let canonical =
        core_cmd::handle_open_photo(&file_path, &settings).map_err(|e| e.to_string())?;
    open_with_opener(&app, &canonical)
}

/// 写真の親 directory を Explorer で開く。
#[tauri::command]
pub async fn open_photo_folder(
    file_path: PathBuf,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let settings = state.settings.snapshot();
    let canonical =
        core_cmd::handle_open_photo_folder(&file_path, &settings).map_err(|e| e.to_string())?;
    open_with_opener(&app, &canonical)
}

/// 現在の settings snapshot を返す。
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    core_cmd::handle_get_settings(&state.settings.snapshot())
}

/// settings を atomic に保存する。SettingsWriter actor 経由で逐列化される。
///
/// 失敗時 (I/O エラー等) は文字列化して返す。
#[tauri::command]
pub async fn save_settings(settings: Settings, state: State<'_, AppState>) -> Result<(), String> {
    state
        .settings
        .save(settings)
        .await
        .map_err(|e| e.to_string())
}

/// 検証済み path を OS の関連付けされたアプリケーションで開く。
///
/// `tauri-plugin-opener` の `app.opener().open_path()` を呼ぶ唯一の場所。
fn open_with_opener(app: &AppHandle, path: &std::path::Path) -> Result<(), String> {
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}
