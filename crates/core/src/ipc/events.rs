//! frontend へ emit するイベント型。
//!
//! Tauri 側で `app.emit("event-name", payload)` する payload の serde 表現。
//! TS 側はイベント名 → payload 型の table を持って受信。

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 健康状態のサマリ。短い間隔 (1〜5 秒) で frontend に push する想定。
///
/// 計算ロジックは `monitor` モジュール (TBD) に置き、ここは純粋なデータコンテナ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthStatus {
    /// `projected_raw_events.status='Pending'` の件数。
    pub backlog_size: u64,
    /// projector が現在 catch-up している遅延の最大秒数。
    pub projector_lag_sec: i64,
    /// SQLite データベースのファイルサイズ (バイト)。
    pub db_size_bytes: u64,
    /// DB 配置ボリュームの空き容量 (バイト)。
    pub free_disk_bytes: u64,
    /// 各指標から導かれる健康レベル。
    pub level: HealthLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HealthLevel {
    /// 全指標が正常範囲内。
    Healthy,
    /// 1 個以上が soft 警告閾値を超過。ingest 継続。
    Warning,
    /// 1 個以上が hard 閾値を超過し ingest 一時停止中。
    Degraded,
}

/// settings.json が corrupt だったことを通知。
///
/// frontend は backup_path をクリップボードコピー可能にし、ユーザーに
/// 「設定が壊れていたのでバックアップを取り、デフォルトで起動しました」
/// と表示する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsCorruptWarning {
    pub backup_path: PathBuf,
    pub reason: String,
}

/// データ DB の置き場所が OneDrive / Roaming 同期下にある可能性を警告。
///
/// Plan 上「OneDrive 配下警告」で Tauri event 経由でフロント警告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OneDriveWarning {
    pub db_path: PathBuf,
    pub detected_indicator: String,
}

/// 復旧不能な corruption を検出。projector が停止し、UI には致命表示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FatalCorruptionEvent {
    pub raw_event_id: i64,
    pub reason: String,
}

/// 単一 ingest tick のサマリ。frontend のリアルタイム画面が直近の進捗を示すために使う。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestProgress {
    pub processed_log_files: u64,
    pub total_raw_events: u64,
    pub last_event_at_utc: Option<DateTime<Utc>>,
}

/// VRChat ログ形式が未知のバージョンを含んでいる可能性。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnknownLogFormatWarning {
    pub vrchat_build: Option<String>,
    pub unparsable_ratio: f64,
}

/// /realtime ページ向けの「今起きたこと」イベント (Phase B)。
///
/// projector が raw event を Done に projection した直後に流す UI 向け要約。
/// raw_log_events / domain table への永続化は同 tx で commit 済みなので、UI が
/// 受け取った後に DB を見れば必ず存在する (eventual の race なし)。
///
/// `naive_local` はログ行のタイムスタンプ。`naive_local` 不在の event は projection
/// 自体 skipped されるので emit されない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LiveLogEvent {
    /// 新しい world に入室開始 (`RoomEntering` 由来)。world_id/instance_id は次の
    /// `RoomJoining` で確定するためここでは name のみ。
    WorldEntering {
        naive_local: String,
        world_name: String,
    },
    /// 入室確定 (world_id + instance_id)。`RoomJoining` 由来。
    WorldJoining {
        naive_local: String,
        world_id: String,
        instance_id: String,
    },
    /// 同居 player 入室 (`PlayerJoined` 由来)。
    PlayerJoined {
        naive_local: String,
        display_name: String,
        user_id: Option<String>,
    },
    /// 同居 player 退室 (`PlayerLeft` 由来)。
    PlayerLeft {
        naive_local: String,
        display_name: String,
        user_id: Option<String>,
    },
    /// 通知受信 (`Notification` 由来)。
    Notification {
        naive_local: String,
        sender: String,
        ntype: String,
    },
    /// 動画 URL 検出 (`VideoUrl` 由来)。
    VideoUrl { naive_local: String, url: String },
}

/// 全イベント名 (TS 側との文字列同期用)。
pub mod names {
    pub const HEALTH_STATUS: &str = "vrcwatchdog://health-status";
    pub const SETTINGS_CORRUPT_WARNING: &str = "vrcwatchdog://settings-corrupt";
    pub const ONEDRIVE_WARNING: &str = "vrcwatchdog://onedrive-warning";
    pub const FATAL_CORRUPTION: &str = "vrcwatchdog://fatal-corruption";
    pub const INGEST_PROGRESS: &str = "vrcwatchdog://ingest-progress";
    pub const UNKNOWN_LOG_FORMAT: &str = "vrcwatchdog://unknown-log-format";
    pub const LIVE_LOG_EVENT: &str = "vrcwatchdog://live-log-event";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_status_round_trips_through_json() {
        let h = HealthStatus {
            backlog_size: 1234,
            projector_lag_sec: 30,
            db_size_bytes: 100_000_000,
            free_disk_bytes: 5_000_000_000,
            level: HealthLevel::Warning,
        };
        let json = serde_json::to_string(&h).expect("ser");
        assert!(json.contains("\"backlogSize\":1234"));
        assert!(json.contains("\"level\":\"warning\""));
        let back: HealthStatus = serde_json::from_str(&json).expect("de");
        assert_eq!(back, h);
    }

    #[test]
    fn settings_corrupt_warning_uses_camel_case() {
        let w = SettingsCorruptWarning {
            backup_path: "C:/x.bak".into(),
            reason: "expected colon".into(),
        };
        let json = serde_json::to_string(&w).expect("ser");
        assert!(json.contains("\"backupPath\""));
    }

    #[test]
    fn event_names_are_namespaced() {
        for name in [
            names::HEALTH_STATUS,
            names::SETTINGS_CORRUPT_WARNING,
            names::ONEDRIVE_WARNING,
            names::FATAL_CORRUPTION,
            names::INGEST_PROGRESS,
            names::UNKNOWN_LOG_FORMAT,
        ] {
            assert!(
                name.starts_with("vrcwatchdog://"),
                "event name {name} must be namespaced"
            );
        }
    }
}
