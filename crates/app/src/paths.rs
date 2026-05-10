//! アプリのデータ配置 + OneDrive 同期 heuristic。
//!
//! プラン §確定方針:
//! - `settings.json` は **`%APPDATA%/VRCWatchDog/`** (Roaming、ユーザー設定向け)
//! - **DB / cache / thumb** は **`%LOCALAPPDATA%/VRCWatchDog/`** (Local、roaming/OneDrive 同期回避)
//!
//! [`AppPaths::detect_db_sync_risk`] は DB path に `OneDrive` または `Roaming` が
//! 含まれていたら [`DbSyncRisk`] を返す。`OneDriveWarning` event のソースになる
//! (`core::ipc::events`)。
//!
//! テストで本物の `%APPDATA%` を触らないよう、[`AppPaths::with_bases`] で base
//! directory を注入できる。production 起動は [`AppPaths::from_env`]。

use std::env;
use std::io;
use std::path::{Path, PathBuf};

/// app 内で使う標準パス一式。
///
/// 構築直後は **fs に何も触らない**。dir 作成は [`AppPaths::ensure_dirs`] で明示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    /// `%APPDATA%/VRCWatchDog`
    pub roaming_dir: PathBuf,
    /// `%LOCALAPPDATA%/VRCWatchDog`
    pub local_dir: PathBuf,
    /// `%APPDATA%/VRCWatchDog/settings.json`
    pub settings_path: PathBuf,
    /// `%LOCALAPPDATA%/VRCWatchDog/vrcwatchdog.db`
    pub db_path: PathBuf,
    /// `%LOCALAPPDATA%/VRCWatchDog/cache`
    pub cache_dir: PathBuf,
    /// `%LOCALAPPDATA%/VRCWatchDog/cache/thumbs` — `tauri.conf.json` の asset scope と一致。
    pub thumb_dir: PathBuf,
}

impl AppPaths {
    /// `%APPDATA%` / `%LOCALAPPDATA%` から構築する (Windows production)。
    pub fn from_env() -> Result<Self, PathsError> {
        let appdata = read_env_var("APPDATA").ok_or(PathsError::MissingAppData)?;
        let local = read_env_var("LOCALAPPDATA").ok_or(PathsError::MissingLocalAppData)?;
        Ok(Self::with_bases(appdata, local))
    }

    /// 任意 base から構築する (tests / 移行検証 / 開発用)。
    ///
    /// `roaming_base` / `local_base` は `VRCWatchDog/` を **含まない** 親 directory。
    pub fn with_bases(roaming_base: PathBuf, local_base: PathBuf) -> Self {
        let roaming_dir = roaming_base.join(Self::APP_DIR_NAME);
        let local_dir = local_base.join(Self::APP_DIR_NAME);
        let settings_path = roaming_dir.join("settings.json");
        let db_path = local_dir.join("vrcwatchdog.db");
        let cache_dir = local_dir.join("cache");
        let thumb_dir = cache_dir.join("thumbs");
        Self {
            roaming_dir,
            local_dir,
            settings_path,
            db_path,
            cache_dir,
            thumb_dir,
        }
    }

    /// 必要 dir を全部作る。冪等。
    ///
    /// settings の親 / DB の親 / thumb までは保証する。settings.json と DB ファイル
    /// 自体は呼び出し側 (`load_settings` / `db::open`) が必要に応じて作る。
    pub fn ensure_dirs(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.roaming_dir)?;
        std::fs::create_dir_all(&self.local_dir)?;
        std::fs::create_dir_all(&self.cache_dir)?;
        std::fs::create_dir_all(&self.thumb_dir)?;
        Ok(())
    }

    /// DB path が OneDrive / Roaming sync 配下にあるかを heuristic 判定。
    ///
    /// プラン §確定方針 "OneDrive 配下警告": DB は `%LOCALAPPDATA%` を期待しているので、
    /// path に `OneDrive` または `Roaming` 部分が混入したら設定ミス or %LOCALAPPDATA%
    /// redirect の可能性。`OneDriveWarning` event を emit する根拠にする。
    ///
    /// settings は %APPDATA% (=Roaming 配下) なので Roaming 判定はしない。
    pub fn detect_db_sync_risk(&self) -> Option<DbSyncRisk> {
        detect_sync_risk(&self.db_path)
    }

    const APP_DIR_NAME: &'static str = "VRCWatchDog";
}

/// VRChat の標準ログディレクトリ。`%USERPROFILE%\AppData\LocalLow\VRChat\VRChat`。
///
/// `settings.log_directory` が未設定の起動時に fallback として使う想定。
/// `%USERPROFILE%` が無い環境 (= テスト環境を想定) では `None` を返す。
/// **fs に触らない** ので、戻り値の path は存在しないこともある (呼び側で `is_dir()` 確認)。
pub fn default_vrchat_log_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE").map(|home| {
        PathBuf::from(home)
            .join("AppData")
            .join("LocalLow")
            .join("VRChat")
            .join("VRChat")
    })
}

/// DB が roaming/OneDrive 同期下にある可能性。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbSyncRisk {
    pub db_path: PathBuf,
    /// path に検出された substring (`"OneDrive"` または `"Roaming"`)。`OneDriveWarning.detected_indicator`
    /// にそのまま流す前提で文字列のまま持つ。
    pub indicator: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    #[error("APPDATA environment variable is missing")]
    MissingAppData,
    #[error("LOCALAPPDATA environment variable is missing")]
    MissingLocalAppData,
}

/// 空文字列を `Some("")` ではなく `None` 扱いにする。CI で `APPDATA=""` を仕込まれた
/// 場合の sanity check。
fn read_env_var(name: &str) -> Option<PathBuf> {
    env::var_os(name).and_then(|v| {
        if v.is_empty() {
            None
        } else {
            Some(PathBuf::from(v))
        }
    })
}

/// path 文字列に OneDrive / Roaming セグメントが混じっていたら risk を返す。
///
/// 比較は lower-case 化した上でディレクトリ区切り (`/` `\`) を挟む形で行う。
/// 単に `contains("onedrive")` だと `OneDriveBackup` のような無関係 dir 名にも
/// 誤検出するため、必ず separator で挟まれた完全 segment 一致を見る。
fn detect_sync_risk(path: &Path) -> Option<DbSyncRisk> {
    // OS に依らず両 separator を統一形 (`/`) に直してから segment を見る。
    let normalized = path.to_string_lossy().replace('\\', "/").to_lowercase();

    for indicator in ["OneDrive", "Roaming"] {
        let needle = indicator.to_lowercase();
        // 段の前後どちらかは必ず separator。先頭/末尾 segment にも対応するため
        // start-with / end-with もチェックする。
        let with_slashes = format!("/{needle}/");
        let starts = normalized.starts_with(&format!("{needle}/"));
        let ends = normalized.ends_with(&format!("/{needle}"));
        if normalized.contains(&with_slashes) || starts || ends {
            return Some(DbSyncRisk {
                db_path: path.to_path_buf(),
                indicator: indicator.to_string(),
            });
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_paths(roaming: &Path, local: &Path) -> AppPaths {
        AppPaths::with_bases(roaming.to_path_buf(), local.to_path_buf())
    }

    #[test]
    fn with_bases_builds_expected_subpaths() {
        let p = AppPaths::with_bases("C:/r".into(), "C:/l".into());
        assert_eq!(p.roaming_dir, PathBuf::from("C:/r/VRCWatchDog"));
        assert_eq!(p.local_dir, PathBuf::from("C:/l/VRCWatchDog"));
        assert_eq!(
            p.settings_path,
            PathBuf::from("C:/r/VRCWatchDog/settings.json")
        );
        assert_eq!(p.db_path, PathBuf::from("C:/l/VRCWatchDog/vrcwatchdog.db"));
        assert_eq!(p.cache_dir, PathBuf::from("C:/l/VRCWatchDog/cache"));
        assert_eq!(p.thumb_dir, PathBuf::from("C:/l/VRCWatchDog/cache/thumbs"));
    }

    #[test]
    fn ensure_dirs_creates_all_required_directories() {
        let r = tempdir().unwrap();
        let l = tempdir().unwrap();
        let p = make_paths(r.path(), l.path());
        p.ensure_dirs().unwrap();
        assert!(p.roaming_dir.is_dir());
        assert!(p.local_dir.is_dir());
        assert!(p.cache_dir.is_dir());
        assert!(p.thumb_dir.is_dir());
        // settings.json / db file 自体は作らない
        assert!(!p.settings_path.exists());
        assert!(!p.db_path.exists());
    }

    #[test]
    fn ensure_dirs_is_idempotent() {
        let r = tempdir().unwrap();
        let l = tempdir().unwrap();
        let p = make_paths(r.path(), l.path());
        p.ensure_dirs().unwrap();
        // 2 回呼んでもエラーにならない
        p.ensure_dirs().unwrap();
    }

    #[test]
    fn detect_db_sync_risk_returns_none_for_clean_localappdata() {
        let p = AppPaths::with_bases(
            "C:/Users/Foo/AppData/Roaming".into(),
            "C:/Users/Foo/AppData/Local".into(),
        );
        // db_path = C:/Users/Foo/AppData/Local/VRCWatchDog/vrcwatchdog.db
        // OneDrive も Roaming も db_path 部分に segment として無いので None
        assert_eq!(p.detect_db_sync_risk(), None);
    }

    #[test]
    fn detect_db_sync_risk_flags_onedrive_segment() {
        let p = AppPaths::with_bases(
            "C:/Users/Foo/AppData/Roaming".into(),
            "C:/Users/Foo/OneDrive/AppData/Local".into(),
        );
        let risk = p.detect_db_sync_risk().expect("must flag onedrive");
        assert_eq!(risk.indicator, "OneDrive");
        assert_eq!(risk.db_path, p.db_path);
    }

    #[test]
    fn detect_db_sync_risk_flags_roaming_segment_in_localappdata() {
        // %LOCALAPPDATA% が誤って Roaming 配下に redirect されたケース。
        let p = AppPaths::with_bases(
            "C:/Users/Foo/AppData/Roaming".into(),
            "C:/Users/Foo/AppData/Roaming/Local".into(),
        );
        let risk = p.detect_db_sync_risk().expect("must flag roaming");
        assert_eq!(risk.indicator, "Roaming");
    }

    #[test]
    fn detect_db_sync_risk_does_not_falsepositive_on_substring() {
        // "OneDriveBackup" のような無関係名で誤検出しない。
        let p = AppPaths::with_bases(
            "C:/Users/Foo/AppData/Roaming".into(),
            "C:/Users/Foo/OneDriveBackup/Local".into(),
        );
        assert_eq!(p.detect_db_sync_risk(), None);
    }

    #[test]
    fn detect_db_sync_risk_handles_backslash_paths() {
        let p = AppPaths::with_bases(
            r"C:\Users\Foo\AppData\Roaming".into(),
            r"C:\Users\Foo\OneDrive\AppData\Local".into(),
        );
        let risk = p.detect_db_sync_risk().expect("must flag onedrive");
        assert_eq!(risk.indicator, "OneDrive");
    }

    #[test]
    fn detect_db_sync_risk_is_case_insensitive() {
        let p = AppPaths::with_bases(
            "C:/Users/Foo/AppData/Roaming".into(),
            "C:/Users/Foo/onedrive/AppData/Local".into(),
        );
        let risk = p
            .detect_db_sync_risk()
            .expect("must flag onedrive lowercase");
        assert_eq!(risk.indicator, "OneDrive");
    }

    #[test]
    fn default_vrchat_log_dir_returns_localllow_path_under_userprofile() {
        // CI も dev 機もどちらも USERPROFILE は設定済み前提 (Windows の標準環境変数)。
        // Linux runner では USERPROFILE が無いので None になるが、それも仕様。
        if env::var_os("USERPROFILE").is_none() {
            // Linux など: None を返すこと
            assert!(default_vrchat_log_dir().is_none());
        } else {
            let p = default_vrchat_log_dir().expect("USERPROFILE 設定済みなら Some");
            let s = p.to_string_lossy();
            // separator は OS によって \ or / で変わるので contains で寛容に比較。
            let normalized = s.replace('\\', "/");
            assert!(
                normalized.ends_with("/AppData/LocalLow/VRChat/VRChat"),
                "got {s}"
            );
        }
    }

    #[test]
    fn read_env_var_treats_empty_as_missing() {
        // 単一スレッド前提で env を触る。他テストと衝突しないよう一意な name を使う。
        // edition 2021 では env::set_var は safe。
        let name = "VRCWATCHDOG_TEST_EMPTY_ENV_VAR";
        env::set_var(name, "");
        assert!(read_env_var(name).is_none());
        env::set_var(name, "C:/x");
        assert_eq!(read_env_var(name), Some(PathBuf::from("C:/x")));
        env::remove_var(name);
        assert!(read_env_var(name).is_none());
    }
}
