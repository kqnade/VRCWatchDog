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

use crate::photo::{validate_photo_path, PhotoAccessError, PhotoTarget};
use crate::settings::Settings;

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
    fn get_settings_returns_clone() {
        let s = Settings {
            locale: "en".into(),
            ..Settings::default()
        };
        let got = handle_get_settings(&s);
        assert_eq!(got, s);
    }
}
