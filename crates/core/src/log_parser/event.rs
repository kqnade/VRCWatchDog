use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// VRChat ログ 1 行から抽出した意味イベント。
///
/// バリアントは raw_log_events.event_type に保存する文字列タグと対応する。
/// 不明・無効な行は [`Self::UnparsableLine`] に落として raw 永続化する
/// (パーサーバージョン互換性のため捨てない)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LogEvent {
    /// `[Behaviour] Entering Room: <name>` — ワールド名のみ判明、world_id/instance_id は次行で確定。
    RoomEntering { world_name: String },
    /// `[Behaviour] Joining wrld_xxx:nonce~...` — world_id と instance_id が確定。
    RoomJoining {
        world_id: String,
        instance_id: String,
    },
    /// `User Authenticated: <name>` — 自分自身の認証完了。
    UserAuthenticated { display_name: String },
    /// `[Behaviour] OnPlayerJoined <DisplayName> (usr_xxx)`
    PlayerJoined {
        display_name: String,
        user_id: Option<String>,
    },
    /// `[Behaviour] OnPlayerLeft <DisplayName> (usr_xxx)`
    PlayerLeft {
        display_name: String,
        user_id: Option<String>,
    },
    /// `Received Notification: ... from username:<sender>, ... type:<type>`
    Notification { sender: String, ntype: String },
    /// `[Video Playback] ... Attempting to resolve URL '<url>'`
    VideoUrl { url: String },
    /// パース不能 (regex 該当なし、不正 UTF-8 等)。raw に残しつつドメイン射影はスキップ。
    UnparsableLine { reason: String },
}

impl LogEvent {
    pub fn type_tag(&self) -> &'static str {
        match self {
            Self::RoomEntering { .. } => "RoomEntering",
            Self::RoomJoining { .. } => "RoomJoining",
            Self::UserAuthenticated { .. } => "UserAuthenticated",
            Self::PlayerJoined { .. } => "PlayerJoined",
            Self::PlayerLeft { .. } => "PlayerLeft",
            Self::Notification { .. } => "Notification",
            Self::VideoUrl { .. } => "VideoUrl",
            Self::UnparsableLine { .. } => "UnparsableLine",
        }
    }
}

/// パース済みログ 1 行。
///
/// `naive_local` はログ冒頭のタイムスタンプ (`yyyy.MM.dd HH:mm:ss`)。
/// タイムゾーン情報は持たないため、UTC 解決は [`crate::time::resolve_local_to_utc`] で行う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLine {
    pub naive_local: NaiveDateTime,
    pub event: LogEvent,
}
