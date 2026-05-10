//! Settings 構造体 + load / atomic save の純粋関数。
//!
//! Tokio actor 層 (`writer`) は本モジュールを使うだけで、I/O は全てここに集約する。

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};

/// アプリ全体の永続設定。
///
/// 新フィールドを増やすときは必ず `#[serde(default)]` を付け、旧 `settings.json` でも
/// 読み込めるようにすること。古いキーを削除するときは内部互換のため `#[serde(alias = ...)]`
/// で吸収するか、明示的にマイグレーション関数を書く。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Settings {
    pub log_directory: Option<PathBuf>,
    pub photo_directory: Option<PathBuf>,
    pub thumbnail_cache_dir: Option<PathBuf>,
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default)]
    pub autostart_enabled: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_true")]
    pub notification_enabled: bool,
}

fn default_locale() -> String {
    "ja".to_string()
}
fn default_theme() -> String {
    "dark".to_string()
}
fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            log_directory: None,
            photo_directory: None,
            thumbnail_cache_dir: None,
            locale: default_locale(),
            autostart_enabled: false,
            theme: default_theme(),
            notification_enabled: true,
        }
    }
}

/// load 結果。corrupt の場合はバックアップを作り、default を返している。
#[derive(Debug, Clone, PartialEq)]
pub enum LoadOutcome {
    /// 既存ファイルから正常 load。
    Loaded(Settings),
    /// ファイルが無かったので default を作成。**fs に書き出してはいない**。
    NotFound(Settings),
    /// 既存ファイルが corrupt だったので `.corrupt-{ts}.bak` にバックアップしつつ default を返した。
    /// 元の `settings.json` は触っていないので、必要なら手動復旧可能。
    /// (※ バックアップ作成成功時のみこの variant を返す。バックアップ失敗時は IO エラー)
    CorruptBackedUp {
        settings: Settings,
        backup_path: PathBuf,
        reason: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsLoadError {
    #[error("io error while loading settings: {0}")]
    Io(#[from] io::Error),
    #[error("could not create backup of corrupt settings: {0}")]
    BackupFailed(io::Error),
}

/// `path` から settings を load する。`path` の親 directory は事前に存在している前提。
///
/// - 存在しない: `LoadOutcome::NotFound(Settings::default())` を返す。**書き込まない**。
/// - 存在するが parse 失敗: `.corrupt-{utc_iso8601}.bak` を作成し
///   `LoadOutcome::CorruptBackedUp` を返す。
/// - 存在し parse 成功: `LoadOutcome::Loaded`。
pub fn load_settings(path: &Path) -> Result<LoadOutcome, SettingsLoadError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(LoadOutcome::NotFound(Settings::default()));
        }
        Err(e) => return Err(SettingsLoadError::Io(e)),
    };

    match serde_json::from_slice::<Settings>(&bytes) {
        Ok(settings) => Ok(LoadOutcome::Loaded(settings)),
        Err(parse_err) => {
            // バックアップ作成。原本ファイルは消さない・触らない。
            let backup_path = corrupt_backup_path(path);
            std::fs::write(&backup_path, &bytes).map_err(SettingsLoadError::BackupFailed)?;

            Ok(LoadOutcome::CorruptBackedUp {
                settings: Settings::default(),
                backup_path,
                reason: parse_err.to_string(),
            })
        }
    }
}

fn corrupt_backup_path(original: &Path) -> PathBuf {
    let ts = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let stem = original
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("settings");
    let ext = original
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("json");
    let parent = original.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{stem}.corrupt-{ts}.{ext}.bak"))
}

/// settings を atomic に書く。
///
/// 同一 directory の一時ファイルに pretty JSON を fsync し、`persist` で rename。
/// crash で中断した場合、`.tmp` ゴミは残るが本体は壊れない。
///
/// # Windows のリトライ
///
/// `tempfile::persist` は内部で `MoveFileExW(..., MOVEFILE_REPLACE_EXISTING)` を呼ぶ。
/// Windows では下記の状況で `ERROR_ACCESS_DENIED (5)` または
/// `ERROR_SHARING_VIOLATION (32)` が一時的に返る:
///
/// - 別の rename がちょうど同じ宛先を置換中で、open handle が刹那的に残っている
/// - Windows Defender / 他 AV が write 直後にスキャンのため open している
///
/// production の `SettingsWriter` actor は逐列化されているのでまず起きないが、
/// プラン §4 の「100 並列 save で破損なし」を担保するため、retryable な OS error は
/// 50ms 間隔で最大 19 回 (= ~1s) リトライする。POSIX の `rename(2)` は atomic で
/// これらの error は発生しないので、retry は実質 no-op。
pub fn save_settings_atomic(path: &Path, settings: &Settings) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "settings path has no parent directory",
        )
    })?;
    std::fs::create_dir_all(parent)?;

    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.as_file_mut().write_all(&bytes)?;
    tmp.as_file_mut().sync_all()?;

    let mut current = tmp;
    let mut attempt: u32 = 0;
    const MAX_ATTEMPTS: u32 = 20;
    const BACKOFF: Duration = Duration::from_millis(50);
    loop {
        match current.persist(path) {
            Ok(_) => return Ok(()),
            Err(persist_err) => {
                attempt += 1;
                if attempt >= MAX_ATTEMPTS || !is_retryable_persist_error(&persist_err.error) {
                    return Err(persist_err.error);
                }
                std::thread::sleep(BACKOFF);
                current = persist_err.file;
            }
        }
    }
}

/// `persist` がリトライで回復しうるかを判定する。
///
/// Windows の `MoveFileExW` が返す ACCESS_DENIED (5) / SHARING_VIOLATION (32) は
/// 一時的な open handle 競合に起因し、短い待機で解消する。
/// それ以外の `PermissionDenied` (本物の権限不足) は raw_os_error を見て除外しないと
/// 永久にハングするので注意。
fn is_retryable_persist_error(err: &io::Error) -> bool {
    match err.raw_os_error() {
        // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION
        Some(5) | Some(32) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn settings_path(dir: &TempDir) -> PathBuf {
        dir.path().join("settings.json")
    }

    #[test]
    fn defaults_are_sensible() {
        let s = Settings::default();
        assert_eq!(s.locale, "ja");
        assert_eq!(s.theme, "dark");
        assert!(!s.autostart_enabled);
        assert!(s.notification_enabled);
    }

    #[test]
    fn missing_file_returns_not_found_without_writing() {
        let dir = TempDir::new().expect("tempdir");
        let path = settings_path(&dir);
        let outcome = load_settings(&path).expect("load ok");
        match outcome {
            LoadOutcome::NotFound(s) => assert_eq!(s, Settings::default()),
            other => panic!("expected NotFound, got {other:?}"),
        }
        assert!(!path.exists(), "load must not write a default file");
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = TempDir::new().expect("tempdir");
        let path = settings_path(&dir);
        let s = Settings {
            locale: "en".into(),
            autostart_enabled: true,
            ..Settings::default()
        };
        save_settings_atomic(&path, &s).expect("save");

        match load_settings(&path).expect("load ok") {
            LoadOutcome::Loaded(loaded) => assert_eq!(loaded, s),
            other => panic!("expected Loaded, got {other:?}"),
        }
    }

    #[test]
    fn corrupt_file_is_backed_up_and_default_returned_without_overwriting() {
        let dir = TempDir::new().expect("tempdir");
        let path = settings_path(&dir);
        std::fs::write(&path, b"{ this is : not json }").expect("write garbage");
        let original_bytes = std::fs::read(&path).expect("read original");

        let outcome = load_settings(&path).expect("load ok despite corrupt");
        match outcome {
            LoadOutcome::CorruptBackedUp {
                settings,
                backup_path,
                ..
            } => {
                assert_eq!(settings, Settings::default());
                assert!(backup_path.exists(), "backup must exist");
                let backup_bytes = std::fs::read(&backup_path).expect("read backup");
                assert_eq!(backup_bytes, original_bytes, "backup preserves original");
                let after = std::fs::read(&path).expect("read original after");
                assert_eq!(after, original_bytes, "original must not be overwritten");
            }
            other => panic!("expected CorruptBackedUp, got {other:?}"),
        }
    }

    #[test]
    fn unknown_extra_keys_are_tolerated() {
        // 古いユーザーが残した未知のキーは serde が無視する (default 動作)。
        let dir = TempDir::new().expect("tempdir");
        let path = settings_path(&dir);
        std::fs::write(
            &path,
            br#"{ "locale": "en", "theme": "dark", "old_key": "ignored" }"#,
        )
        .expect("write");
        match load_settings(&path).expect("load") {
            LoadOutcome::Loaded(s) => assert_eq!(s.locale, "en"),
            other => panic!("expected Loaded, got {other:?}"),
        }
    }

    #[test]
    fn missing_optional_keys_use_defaults() {
        let dir = TempDir::new().expect("tempdir");
        let path = settings_path(&dir);
        std::fs::write(
            &path,
            br#"{ "log_directory": null, "photo_directory": null }"#,
        )
        .expect("write");
        match load_settings(&path).expect("load") {
            LoadOutcome::Loaded(s) => {
                assert_eq!(s.locale, "ja");
                assert_eq!(s.theme, "dark");
                assert!(!s.autostart_enabled);
                assert!(s.notification_enabled);
            }
            other => panic!("expected Loaded, got {other:?}"),
        }
    }

    #[test]
    fn atomic_save_does_not_corrupt_on_concurrent_writers() {
        // 100 並列 save。tempfile による rename atomicity を確認。
        // すべて完走後、最終結果は完全な JSON として再 load できる。
        use std::sync::Arc;
        let dir = Arc::new(TempDir::new().expect("tempdir"));
        let path = Arc::new(settings_path(&dir));

        let mut handles = Vec::new();
        for i in 0..100 {
            let path = Arc::clone(&path);
            handles.push(std::thread::spawn(move || {
                let s = Settings {
                    locale: if i % 2 == 0 { "ja".into() } else { "en".into() },
                    autostart_enabled: i % 3 == 0,
                    ..Settings::default()
                };
                save_settings_atomic(&path, &s)
            }));
        }
        for h in handles {
            h.join().expect("thread panic").expect("save ok");
        }

        match load_settings(&path).expect("final load") {
            LoadOutcome::Loaded(s) => {
                assert!(matches!(s.locale.as_str(), "ja" | "en"));
            }
            other => panic!("file must be parseable after concurrent saves, got {other:?}"),
        }
    }

    #[test]
    fn corrupt_backup_filename_includes_timestamp() {
        let dir = TempDir::new().expect("tempdir");
        let path = settings_path(&dir);
        let bp = corrupt_backup_path(&path);
        let name = bp.file_name().and_then(|s| s.to_str()).expect("utf8");
        assert!(name.starts_with("settings.corrupt-"));
        assert!(name.ends_with(".json.bak"));
    }
}
