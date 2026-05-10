//! Tauri から `state()` で参照する共有 state。
//!
//! - 各種 actor handle (現状 `SettingsHandle` のみ。Phase 6 以降で thumb_writer 等を追加)
//! - DB pool (read 用にどのコマンドからも借りる)
//! - paths (settings save 先 / DB 場所 / asset scope の整合確認用)
//! - 起動時に検出した「emit を遅延すべき警告」(`SettingsCorruptWarning` /
//!   `OneDriveWarning`)。Builder の `.setup()` callback で `app.emit(...)` するため
//!   bootstrap が detection 結果だけ持ち越して保存する。

use std::path::PathBuf;

use sqlx::SqlitePool;
use vrcwatchdog_core::settings::SettingsHandle;

use crate::paths::{AppPaths, DbSyncRisk};

/// Tauri 全コマンドが共有する状態。
///
/// Tauri の `manage()` に渡すと `tauri::State<'_, AppState>` 経由で borrow できる。
pub struct AppState {
    pub paths: AppPaths,
    pub settings: SettingsHandle,
    pub db_pool: SqlitePool,

    /// `load_settings` で corrupt 検出された場合のバックアップパスと理由。
    /// `setup()` 中に `SettingsCorruptWarning` を emit したら **None には戻さない**
    /// (UI 側で「設定が壊れたまま起動した」を継続的に出せるように)。
    pub settings_corrupt: Option<SettingsCorruptInfo>,

    /// DB が OneDrive / Roaming sync 配下にある可能性。
    /// `setup()` 中に `OneDriveWarning` を emit するソース。
    pub db_sync_risk: Option<DbSyncRisk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsCorruptInfo {
    pub backup_path: PathBuf,
    pub reason: String,
}
