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

use crate::db::repo::notification_records::NotificationRecord;
use crate::db::repo::photo_records::PhotoRecord;
use crate::db::repo::video_records::VideoRecord;
use crate::db::repo::world_visits::VisitWithCounts;
use crate::ipc::events::{OneDriveWarning, SettingsCorruptWarning};
use crate::photo::{validate_photo_path, PhotoAccessError, PhotoTarget};
use crate::settings::Settings;
use crate::time::format_duration_hms;

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
    /// thumb_sha が Some の場合のみ Some。`<thumb_dir>/<sha>.webp` の絶対パス。
    /// frontend は `convertFileSrc(thumb_path)` で asset:// URL に変換して `<img>` に貼る。
    /// thumb がまだ生成されていなければ None (UI 側で placeholder 表示)。
    pub thumb_path: Option<PathBuf>,
    pub world_visit_id: Option<i64>,
    /// `world_visits.world_name` を LEFT JOIN で引いた値。`world_visit_id` が None なら
    /// 必ず None。photo_grid のカードに表示し、クリックで /history の visit に遷移する。
    pub world_name: Option<String>,
}

impl PhotoRecordDto {
    /// `PhotoRecord` を DTO に変換する。`thumb_dir` を渡せば `thumb_sha` から
    /// 絶対パス `thumb_path` を組み立てる。`thumb_dir = None` の場合は path も None。
    pub fn from_record(r: PhotoRecord, thumb_dir: Option<&Path>) -> Self {
        let thumb_path = match (r.thumb_sha.as_ref(), thumb_dir) {
            (Some(sha), Some(dir)) => Some(dir.join(format!("{sha}.webp"))),
            _ => None,
        };
        Self {
            id: r.id,
            file_path: r.file_path,
            file_name: r.file_name,
            taken_naive_local: r.taken_naive_local.format("%Y-%m-%d %H:%M:%S").to_string(),
            taken_utc: r.taken_utc,
            thumb_sha: r.thumb_sha,
            thumb_path,
            world_visit_id: r.world_visit_id,
            world_name: r.world_name,
        }
    }
}

impl From<PhotoRecord> for PhotoRecordDto {
    /// `thumb_dir` 不明での fallback。`thumb_path` は常に `None`。
    /// 通常は `PhotoRecordDto::from_record(record, Some(thumb_dir))` を使うべき。
    fn from(r: PhotoRecord) -> Self {
        Self::from_record(r, None)
    }
}

/// `list_recent_videos` command の戻り値要素。/videos 画面用。
///
/// `title` / `thumbnail_url` / `thumbnail_sha` は Phase D の video_info actor が
/// 後で fetch して埋める。`thumbnail_path` は actor が `<thumb_dir>/<sha>.webp` を
/// 書き込んだ後の絶対パスで、frontend が `convertFileSrc()` で asset:// に変換して img 表示する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoDto {
    pub id: i64,
    pub url: String,
    pub title: Option<String>,
    pub thumbnail_url: Option<String>,
    pub thumbnail_sha: Option<String>,
    /// thumbnail_sha が Some かつ caller が thumb_dir を渡したときのみ Some。
    /// photo_records と同じ `<thumb_dir>/<sha>.webp` パターン。
    pub thumbnail_path: Option<PathBuf>,
    pub detected_naive_local: String,
    pub detected_utc: DateTime<Utc>,
    pub world_visit_id: Option<i64>,
}

impl VideoDto {
    /// `VideoRecord` を DTO に変換し、`thumb_dir` 渡しなら `thumbnail_path` を組み立てる。
    pub fn from_record(r: VideoRecord, thumb_dir: Option<&Path>) -> Self {
        let thumbnail_path = match (r.thumbnail_sha.as_ref(), thumb_dir) {
            (Some(sha), Some(dir)) => Some(dir.join(format!("{sha}.webp"))),
            _ => None,
        };
        Self {
            id: r.id,
            url: r.url,
            title: r.title,
            thumbnail_url: r.thumbnail_url,
            thumbnail_sha: r.thumbnail_sha,
            thumbnail_path,
            detected_naive_local: r
                .detected_naive_local
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            detected_utc: r.detected_utc,
            world_visit_id: r.world_visit_id,
        }
    }
}

impl From<VideoRecord> for VideoDto {
    /// thumb_dir 不明 fallback (`thumbnail_path` は常に `None`)。通常は
    /// `from_record(record, Some(thumb_dir))` を使う。
    fn from(r: VideoRecord) -> Self {
        Self::from_record(r, None)
    }
}

/// `list_recent_notifications` command の戻り値要素。/notifications 画面用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationDto {
    pub id: i64,
    pub received_naive_local: String,
    pub received_utc: DateTime<Utc>,
    pub sender_name: String,
    pub notification_type: String,
    pub world_visit_id: Option<i64>,
}

impl From<NotificationRecord> for NotificationDto {
    fn from(r: NotificationRecord) -> Self {
        Self {
            id: r.id,
            received_naive_local: r
                .received_naive_local
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            received_utc: r.received_utc,
            sender_name: r.sender_name,
            notification_type: r.notification_type,
            world_visit_id: r.world_visit_id,
        }
    }
}

/// `list_recent_visits` command の戻り値要素。activity_history 画面用。
///
/// `duration` は `format_duration_hms` の出力 (`HH:MM:SS`、24h+ 対応)。
/// `left_utc` が `None` (= まだ離室していない) の場合、`duration` は `"ongoing"` という
/// マーカー文字列を返す (frontend がアイコン表示等に使う)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisitDto {
    pub id: i64,
    pub world_id: Option<String>,
    pub world_name: String,
    pub joined_utc: DateTime<Utc>,
    pub left_utc: Option<DateTime<Utc>>,
    pub resolution_state: String,
    pub photo_count: i64,
    /// 同 visit に居た player の unique 数 (`COUNT(DISTINCT COALESCE(user_id, display_name))`)。
    pub player_count: i64,
    /// `HH:MM:SS` または `"ongoing"`。
    pub duration: String,
}

impl From<VisitWithCounts> for VisitDto {
    fn from(v: VisitWithCounts) -> Self {
        let duration = match v.left_utc {
            Some(left) => format_duration_hms(left - v.joined_utc),
            None => "ongoing".to_string(),
        };
        Self {
            id: v.id,
            world_id: v.world_id,
            world_name: v.world_name,
            joined_utc: v.joined_utc,
            left_utc: v.left_utc,
            resolution_state: v.resolution_state,
            photo_count: v.photo_count,
            player_count: v.player_count,
            duration,
        }
    }
}

/// `get_self_player` command の戻り値。
/// 1 度も VRChat にログインしていなければ `display_name` は None で返す。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfPlayerDto {
    pub display_name: Option<String>,
    /// 直近の `User Authenticated` イベント発生時刻 (UTC)。display_name と対になる。
    pub authenticated_utc: Option<DateTime<Utc>>,
}

impl SelfPlayerDto {
    pub fn empty() -> Self {
        Self {
            display_name: None,
            authenticated_utc: None,
        }
    }
}

impl From<crate::db::repo::self_player_records::SelfPlayerRecord> for SelfPlayerDto {
    fn from(r: crate::db::repo::self_player_records::SelfPlayerRecord) -> Self {
        Self {
            display_name: Some(r.display_name),
            authenticated_utc: Some(r.authenticated_utc),
        }
    }
}

/// `list_players_for_visit` command の戻り値要素。/history visit 詳細パネル用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerSessionDto {
    pub id: i64,
    pub display_name: String,
    pub user_id: Option<String>,
    pub joined_utc: DateTime<Utc>,
    pub left_utc: Option<DateTime<Utc>>,
}

impl From<crate::db::repo::player_sessions::PlayerSessionView> for PlayerSessionDto {
    fn from(v: crate::db::repo::player_sessions::PlayerSessionView) -> Self {
        Self {
            id: v.id,
            display_name: v.display_name,
            user_id: v.user_id,
            joined_utc: v.joined_utc,
            left_utc: v.left_utc,
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
            thumb_path: None,
            world_visit_id: Some(7),
            world_name: Some("TestWorld".into()),
        };

        let json = serde_json::to_string(&dto).expect("ser");

        assert!(json.contains("\"id\":42"));
        assert!(json.contains("\"filePath\""));
        assert!(json.contains("\"fileName\""));
        assert!(json.contains("\"takenNaiveLocal\":\"2026-05-10 12:34:56\""));
        assert!(json.contains("\"takenUtc\""));
        assert!(json.contains("\"thumbSha\":null"));
        assert!(json.contains("\"thumbPath\":null"));
        assert!(json.contains("\"worldVisitId\":7"));
        assert!(json.contains("\"worldName\":\"TestWorld\""));
    }

    #[test]
    fn photo_record_dto_from_record_builds_thumb_path_from_thumb_dir_and_sha() {
        // Phase 6.3.4: thumb_sha + thumb_dir → 絶対パス組み立てロジック。
        use crate::db::repo::photo_records::PhotoRecord;
        use chrono::NaiveDate;
        let record = PhotoRecord {
            id: 1,
            file_path: PathBuf::from("C:/photos/x.png"),
            file_name: "x.png".into(),
            taken_naive_local: NaiveDate::from_ymd_opt(2026, 5, 10)
                .expect("valid date")
                .and_hms_opt(12, 0, 0)
                .expect("valid time"),
            taken_utc: chrono::TimeZone::from_utc_datetime(
                &chrono::Utc,
                &NaiveDate::from_ymd_opt(2026, 5, 10)
                    .expect("valid date")
                    .and_hms_opt(3, 0, 0)
                    .expect("valid time"),
            ),
            thumb_sha: Some("abc123".into()),
            world_visit_id: None,
            world_name: None,
        };

        let dto = PhotoRecordDto::from_record(record, Some(Path::new("C:/cache/thumbs")));

        assert_eq!(
            dto.thumb_path,
            Some(PathBuf::from("C:/cache/thumbs/abc123.webp"))
        );
    }

    #[test]
    fn photo_record_dto_from_record_returns_none_thumb_path_when_thumb_sha_missing() {
        use crate::db::repo::photo_records::PhotoRecord;
        use chrono::NaiveDate;
        let record = PhotoRecord {
            id: 1,
            file_path: PathBuf::from("C:/photos/x.png"),
            file_name: "x.png".into(),
            taken_naive_local: NaiveDate::from_ymd_opt(2026, 5, 10)
                .expect("valid date")
                .and_hms_opt(12, 0, 0)
                .expect("valid time"),
            taken_utc: chrono::TimeZone::from_utc_datetime(
                &chrono::Utc,
                &NaiveDate::from_ymd_opt(2026, 5, 10)
                    .expect("valid date")
                    .and_hms_opt(3, 0, 0)
                    .expect("valid time"),
            ),
            thumb_sha: None, // thumb_writer まだ走ってない
            world_visit_id: None,
            world_name: None,
        };

        let dto = PhotoRecordDto::from_record(record, Some(Path::new("C:/cache/thumbs")));

        assert!(
            dto.thumb_path.is_none(),
            "thumb_sha が None なら path も None"
        );
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
