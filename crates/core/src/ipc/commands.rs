//! frontend → backend コマンドのハンドラ (Tauri 非依存)。
//!
//! Tauri 層はこのモジュールの関数を `#[tauri::command]` で薄くラップする。
//!
//! ```ignore
//! #[tauri::command]
//! async fn open_photo(
//!     file_path: PathBuf,
//!     state: tauri::State<'_, AppState>,
//! ) -> Result<(), String> {
//!     let settings = state.settings.snapshot();
//!     vrcwatchdog_core::ipc::commands::handle_open_photo(&file_path, &settings)
//!         .map_err(|e| e.to_string())?;
//!     state.app.shell().open(...).map_err(|e| e.to_string())?;
//!     Ok(())
//! }
//! ```
//!
//! ハンドラはあくまで「外部 open に渡すパスを返す」/「保存して返す」までを担う。
//! 実際の `app.shell().open()` や `app.emit()` は Tauri 層の責務。

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::repo::photo_records::PhotoRecord;
use crate::ipc::events::{OneDriveWarning, SettingsCorruptWarning};
use crate::photo::{validate_photo_path, PhotoAccessError, PhotoTarget};
use crate::settings::Settings;

/// `list_recent_photos` command の戻り値要素。
///
/// `db::repo::photo_records::PhotoRecord` を frontend 向けに変換した shape。
/// repo 型に直接 serde を付けず DTO を切ることで、DB 列のリネーム / 追加が
/// frontend の interface を直に揺らさないようにしてある。
///
/// `taken_naive_local` は repo の保存フォーマット (`%Y-%m-%d %H:%M:%S`) のまま
/// 文字列で渡す。frontend 側は時刻表示のみで再パースしない設計 (UI が要求するなら
/// 後から ISO 8601 化する別 endpoint を足す)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoRecordDto {
    pub id: i64,
    pub file_path: PathBuf,
    pub file_name: String,
    pub taken_naive_local: String,
    pub taken_utc: DateTime<Utc>,
    pub thumb_sha: Option<String>,
    pub world_visit_id: Option<i64>,
}

impl From<PhotoRecord> for PhotoRecordDto {
    fn from(r: PhotoRecord) -> Self {
        Self {
            id: r.id,
            file_path: r.file_path,
            file_name: r.file_name,
            taken_naive_local: r.taken_naive_local.format("%Y-%m-%d %H:%M:%S").to_string(),
            taken_utc: r.taken_utc,
            thumb_sha: r.thumb_sha,
            world_visit_id: r.world_visit_id,
        }
    }
}

/// `get_initial_warnings` command の戻り値。
///
/// **背景**: 起動時警告 (`SettingsCorruptWarning` / `OneDriveWarning`) を `setup()` で
/// emit すると、frontend の `onMount` listener attach 前に飛んで取りこぼされる
/// (Tauri event は replay されない)。代わりに本 struct を返す command を
/// frontend が onMount 直後に poll する方式にすることで、レース条件を回避する。
///
/// 起動後に新たに発生する警告は引き続き event 経由 (例: 設定変更時に再 corrupt
/// 検出した場合は `vrcwatchdog://settings-corrupt` を emit)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialWarnings {
    pub settings_corrupt: Option<SettingsCorruptWarning>,
    pub db_sync_risk: Option<OneDriveWarning>,
}

/// `open_photo` / `open_photo_folder` 共通のエラー型。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OpenPhotoError {
    /// 設定で `photo_directory` が未設定。UI で設定を促す。
    #[error("photo_directory is not configured in settings")]
    ScopeNotConfigured,
    /// アクセス検証 (拡張子・スコープ・実体) で拒否。
    #[error("photo access rejected: {0}")]
    Access(#[from] PhotoAccessError),
}

/// `open_photo` のハンドラ。Tauri 層は戻ってきた path を `app.shell().open()` に渡す。
///
/// validate_photo_path に委譲。`settings.photo_directory` が未設定なら
/// `ScopeNotConfigured`。
pub fn handle_open_photo(file_path: &Path, settings: &Settings) -> Result<PathBuf, OpenPhotoError> {
    let scope = settings
        .photo_directory
        .as_deref()
        .ok_or(OpenPhotoError::ScopeNotConfigured)?;
    Ok(validate_photo_path(file_path, scope, PhotoTarget::Photo)?)
}

/// `open_photo_folder` のハンドラ。Explorer で写真の親 directory を開く用途。
pub fn handle_open_photo_folder(
    file_path: &Path,
    settings: &Settings,
) -> Result<PathBuf, OpenPhotoError> {
    let scope = settings
        .photo_directory
        .as_deref()
        .ok_or(OpenPhotoError::ScopeNotConfigured)?;
    Ok(validate_photo_path(file_path, scope, PhotoTarget::Folder)?)
}

/// `get_settings` のハンドラ。snapshot を返すだけ。
///
/// 実装が trivial だが、frontend との契約として明示しておく方が良いので残す。
pub fn handle_get_settings(snapshot: &Settings) -> Settings {
    snapshot.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_image(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, b"\x89PNG\r\n\x1a\n").expect("write image");
        p
    }

    fn settings_with_scope(scope: PathBuf) -> Settings {
        Settings {
            photo_directory: Some(scope),
            ..Settings::default()
        }
    }

    #[test]
    fn open_photo_returns_canonical_path_when_scope_configured() {
        let dir = TempDir::new().expect("tempdir");
        let scope = dir.path().to_path_buf();
        let file = write_image(&scope, "shot.png");
        let s = settings_with_scope(scope.clone());

        let got = handle_open_photo(&file, &s).expect("ok");
        assert_eq!(got, fs::canonicalize(&file).expect("canon"));
    }

    #[test]
    fn open_photo_folder_returns_parent() {
        let dir = TempDir::new().expect("tempdir");
        let scope = dir.path().to_path_buf();
        let sub = scope.join("2026-05");
        fs::create_dir_all(&sub).expect("mkdir");
        let file = write_image(&sub, "shot.png");
        let s = settings_with_scope(scope.clone());

        let got = handle_open_photo_folder(&file, &s).expect("ok");
        assert_eq!(got, fs::canonicalize(&sub).expect("canon"));
    }

    #[test]
    fn open_photo_fails_when_scope_not_configured() {
        let dir = TempDir::new().expect("tempdir");
        let file = write_image(dir.path(), "shot.png");
        let s = Settings::default(); // photo_directory = None

        let err = handle_open_photo(&file, &s).expect_err("must fail");
        assert_eq!(err, OpenPhotoError::ScopeNotConfigured);
    }

    #[test]
    fn open_photo_propagates_path_traversal_rejection() {
        let dir = TempDir::new().expect("tempdir");
        let scope = dir.path().join("scope");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&scope).expect("mkdir scope");
        fs::create_dir_all(&outside).expect("mkdir outside");
        let leak = write_image(&outside, "leak.png");
        let s = settings_with_scope(scope);

        let err = handle_open_photo(&leak, &s).expect_err("must fail");
        assert!(matches!(
            err,
            OpenPhotoError::Access(PhotoAccessError::OutsideScope { .. })
        ));
    }

    #[test]
    fn photo_record_dto_serializes_taken_naive_local_as_string_in_camel_case() {
        // PhotoRecordDto は frontend と camelCase で約束しているのを ratify する。
        use chrono::{NaiveDate, TimeZone};
        let dto = PhotoRecordDto {
            id: 42,
            file_path: PathBuf::from("C:/photos/VRChat_2026-05-10_12-34-56.png"),
            file_name: "VRChat_2026-05-10_12-34-56.png".into(),
            taken_naive_local: "2026-05-10 12:34:56".into(),
            taken_utc: Utc.from_utc_datetime(
                &NaiveDate::from_ymd_opt(2026, 5, 10)
                    .expect("valid date")
                    .and_hms_opt(3, 34, 56)
                    .expect("valid time"),
            ),
            thumb_sha: None,
            world_visit_id: Some(7),
        };

        let json = serde_json::to_string(&dto).expect("ser");

        assert!(json.contains("\"id\":42"));
        assert!(json.contains("\"filePath\""));
        assert!(json.contains("\"fileName\""));
        assert!(json.contains("\"takenNaiveLocal\":\"2026-05-10 12:34:56\""));
        assert!(json.contains("\"takenUtc\""));
        assert!(json.contains("\"thumbSha\":null"));
        assert!(json.contains("\"worldVisitId\":7"));
    }

    #[test]
    fn initial_warnings_serializes_to_camel_case() {
        // frontend は camelCase で受け取る前提なので serde rename を確認しておく。
        let w = InitialWarnings {
            settings_corrupt: Some(SettingsCorruptWarning {
                backup_path: PathBuf::from("C:/x.bak"),
                reason: "expected colon".into(),
            }),
            db_sync_risk: Some(OneDriveWarning {
                db_path: PathBuf::from("C:/Users/Foo/OneDrive/db.sqlite"),
                detected_indicator: "OneDrive".into(),
            }),
        };

        let json = serde_json::to_string(&w).expect("ser");

        assert!(json.contains("\"settingsCorrupt\""));
        assert!(json.contains("\"dbSyncRisk\""));
    }

    #[test]
    fn initial_warnings_with_no_warnings_serializes_to_two_nulls() {
        let w = InitialWarnings {
            settings_corrupt: None,
            db_sync_risk: None,
        };

        let json = serde_json::to_string(&w).expect("ser");

        assert_eq!(
            json, r#"{"settingsCorrupt":null,"dbSyncRisk":null}"#,
            "Option::None は serde で null になる (frontend は null チェックする)"
        );
    }

    #[test]
    fn get_settings_returns_clone() {
        let s = Settings {
            locale: "en".into(),
            ..Settings::default()
        };
        let got = handle_get_settings(&s);
        assert_eq!(got, s);
    }
}
