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
    fn ensure_dirs_does_not_error_when_called_twice() {
        let r = tempdir().unwrap();
        let l = tempdir().unwrap();
        let p = make_paths(r.path(), l.path());
        p.ensure_dirs().unwrap();

        let second_call = p.ensure_dirs();

        assert!(
            second_call.is_ok(),
            "ensure_dirs は冪等であるべき (create_dir_all は既存 dir に対して Ok を返す)"
        );
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

    // default_vrchat_log_dir は OS 環境変数 USERPROFILE の有無で挙動が分岐する。
    // 同一テスト内で if 分岐させると assertion の責務がぼやけ、CI のどちらの分岐が
    // 通ったか log で分からなくなる。OS ごとに #[cfg] で別テストにする。

    #[cfg(windows)]
    #[test]
    fn default_vrchat_log_dir_resolves_to_localllow_vrchat_dir_on_windows() {
        // Arrange: Windows では runner / dev 機を問わず USERPROFILE が必ず存在する。
        assert!(
            env::var_os("USERPROFILE").is_some(),
            "前提条件違反: Windows なら USERPROFILE が無い環境では再現性が無いのでテスト不能"
        );

        // Act
        let resolved = default_vrchat_log_dir();

        // Assert
        let path = resolved.expect("Windows + USERPROFILE 在りなら Some 必須");
        let normalized = path.to_string_lossy().replace('\\', "/");
        assert!(
            normalized.ends_with("/AppData/LocalLow/VRChat/VRChat"),
            "VRChat 標準パスで終わるべき: {}",
            path.display()
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn default_vrchat_log_dir_returns_none_when_userprofile_absent() {
        // POSIX 系では USERPROFILE が無いのが通常。`paths.rs` の規約として None を返す。
        // (USERPROFILE を export している POSIX dev 機の存在は無視: 設計上 Windows-only target)。
        if env::var_os("USERPROFILE").is_some() {
            // 万が一 export されていたら skip。テストの責務外。
            return;
        }

        let resolved = default_vrchat_log_dir();

        assert!(resolved.is_none());
    }

    // read_env_var の振る舞いは 3 つに分かれる: 未設定 / 空文字 / 値あり。
    // 同一テスト内で env mutation を 3 回繰り返すと、どの assertion で落ちたかが
    // 不明瞭になる + テスト失敗時の回復に手間取る。1 テスト 1 ケースに分割する。
    //
    // 全テストで衝突しないユニーク env 名を使い、関数末尾で必ず remove する
    // (テスト間の独立性確保)。

    #[test]
    fn read_env_var_returns_none_when_env_var_is_unset() {
        let name = "VRCWATCHDOG_TEST_READ_ENV_VAR_UNSET";
        // 念のため事前 cleanup (前回ランで残ってた場合)
        env::remove_var(name);

        let got = read_env_var(name);

        assert!(got.is_none());
    }

    #[test]
    fn read_env_var_returns_none_when_env_var_is_empty_string() {
        let name = "VRCWATCHDOG_TEST_READ_ENV_VAR_EMPTY";
        env::set_var(name, "");

        let got = read_env_var(name);

        assert!(got.is_none());
        env::remove_var(name); // teardown
    }

    #[test]
    fn read_env_var_returns_pathbuf_when_env_var_has_value() {
        let name = "VRCWATCHDOG_TEST_READ_ENV_VAR_VALUE";
        env::set_var(name, "C:/x");

        let got = read_env_var(name);

        assert_eq!(got, Some(PathBuf::from("C:/x")));
        env::remove_var(name); // teardown
    }
}
