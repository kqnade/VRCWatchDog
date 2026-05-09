use std::sync::LazyLock;

use chrono::NaiveDateTime;
use regex::Regex;

use super::event::{LogEvent, ParsedLine};

/// ログ冒頭のタイムスタンプ: `2026.05.09 21:43:56 ...`
static RE_TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{4}\.\d{2}\.\d{2} \d{2}:\d{2}:\d{2})").expect("RE_TIMESTAMP")
});

/// `[Behaviour] Entering Room: <world_name>`
static RE_ROOM_ENTERING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[Behaviour\] Entering Room: (.+?)\s*$").expect("RE_ROOM_ENTERING")
});

/// `[Behaviour] Joining wrld_xxx:instance_nonce`
static RE_ROOM_JOINING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[Behaviour\] Joining (wrld_[0-9a-fA-F\-]+):(.+?)\s*$").expect("RE_ROOM_JOINING")
});

/// `User Authenticated: <DisplayName>`
static RE_USER_AUTHENTICATED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"User Authenticated: (.+?)\s*$").expect("RE_USER_AUTHENTICATED"));

/// `[Behaviour] OnPlayerJoined <DisplayName>` または `... (usr_xxx)` 付き
static RE_PLAYER_JOINED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[Behaviour\] OnPlayerJoined (.+?)\s*$").expect("RE_PLAYER_JOINED")
});

/// `[Behaviour] OnPlayerLeft <DisplayName>` または `... (usr_xxx)` 付き
static RE_PLAYER_LEFT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[Behaviour\] OnPlayerLeft (.+?)\s*$").expect("RE_PLAYER_LEFT"));

/// `Received Notification: ... from username:<sender>, ... type:<type>`
static RE_NOTIFICATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Received Notification:.*?from username:([^,]+).*?type:(\w+)")
        .expect("RE_NOTIFICATION")
});

/// `[Video Playback] ... Attempting to resolve URL '<url>'`
static RE_VIDEO_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[Video Playback\].*?Attempting to resolve URL '(.+?)'").expect("RE_VIDEO_URL")
});

/// 末尾の `(usr_xxx-...)` を抽出するヘルパー。
static RE_USER_ID_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\((usr_[0-9a-fA-F\-]+)\)\s*$").expect("RE_USER_ID_SUFFIX"));

const TIMESTAMP_FMT: &str = "%Y.%m.%d %H:%M:%S";

/// VRChat ログの 1 行をパースする。
///
/// 不正 UTF-8 や regex 該当なしの場合は [`LogEvent::UnparsableLine`] を返し、
/// 呼び出し側はその行を raw_log_events に永続化する想定。
/// タイムスタンプが取れない行は [`None`] を返す (純粋なヘッダ等で時刻不在のため、
/// 永続化対象外として skip される)。
pub fn parse_line(line: &str) -> Option<ParsedLine> {
    // CRLF / 末尾空白を取り除く。
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return None;
    }

    let ts_caps = RE_TIMESTAMP.captures(line)?;
    let naive_local = NaiveDateTime::parse_from_str(&ts_caps[1], TIMESTAMP_FMT).ok()?;

    let event = classify(line);
    Some(ParsedLine { naive_local, event })
}

fn classify(line: &str) -> LogEvent {
    if let Some(caps) = RE_ROOM_ENTERING.captures(line) {
        return LogEvent::RoomEntering {
            world_name: caps[1].to_string(),
        };
    }
    if let Some(caps) = RE_ROOM_JOINING.captures(line) {
        return LogEvent::RoomJoining {
            world_id: caps[1].to_string(),
            instance_id: caps[2].to_string(),
        };
    }
    if let Some(caps) = RE_USER_AUTHENTICATED.captures(line) {
        return LogEvent::UserAuthenticated {
            display_name: caps[1].to_string(),
        };
    }
    if let Some(caps) = RE_PLAYER_JOINED.captures(line) {
        let (display_name, user_id) = split_user_id(&caps[1]);
        return LogEvent::PlayerJoined {
            display_name,
            user_id,
        };
    }
    if let Some(caps) = RE_PLAYER_LEFT.captures(line) {
        let (display_name, user_id) = split_user_id(&caps[1]);
        return LogEvent::PlayerLeft {
            display_name,
            user_id,
        };
    }
    if let Some(caps) = RE_NOTIFICATION.captures(line) {
        return LogEvent::Notification {
            sender: caps[1].trim().to_string(),
            ntype: caps[2].to_string(),
        };
    }
    if let Some(caps) = RE_VIDEO_URL.captures(line) {
        return LogEvent::VideoUrl {
            url: caps[1].to_string(),
        };
    }
    LogEvent::UnparsableLine {
        reason: "no_pattern_match".to_string(),
    }
}

/// `<DisplayName> (usr_xxx)` から `(DisplayName, Some(usr_xxx))` を取り出す。
/// suffix が無ければ `(input, None)`。
fn split_user_id(raw: &str) -> (String, Option<String>) {
    if let Some(caps) = RE_USER_ID_SUFFIX.captures(raw) {
        let id = caps[1].to_string();
        let stripped = RE_USER_ID_SUFFIX.replace(raw, "").trim().to_string();
        return (stripped, Some(id));
    }
    (raw.trim().to_string(), None)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn nd(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    #[test]
    fn parses_room_entering() {
        let line = "2026.05.09 21:43:56 Log        -  [Behaviour] Entering Room: HomeWorld";
        let p = parse_line(line).unwrap();
        assert_eq!(p.naive_local, nd(2026, 5, 9, 21, 43, 56));
        assert_eq!(
            p.event,
            LogEvent::RoomEntering {
                world_name: "HomeWorld".into()
            }
        );
    }

    #[test]
    fn parses_room_joining_with_instance() {
        let line = "2026.05.09 21:43:57 Log        -  [Behaviour] Joining wrld_abcd1234-ef00-1111-2222-3333abcd5678:12345~hidden(usr_xxx)~region(jp)";
        let p = parse_line(line).unwrap();
        assert_eq!(
            p.event,
            LogEvent::RoomJoining {
                world_id: "wrld_abcd1234-ef00-1111-2222-3333abcd5678".into(),
                instance_id: "12345~hidden(usr_xxx)~region(jp)".into()
            }
        );
    }

    #[test]
    fn parses_user_authenticated() {
        let line = "2026.05.09 21:00:00 Log        -  User Authenticated: kqnade";
        let p = parse_line(line).unwrap();
        assert_eq!(
            p.event,
            LogEvent::UserAuthenticated {
                display_name: "kqnade".into()
            }
        );
    }

    #[test]
    fn parses_player_joined_with_user_id() {
        let line = "2026.05.09 21:50:00 Log        -  [Behaviour] OnPlayerJoined Alice (usr_11111111-2222-3333-4444-555566667777)";
        let p = parse_line(line).unwrap();
        assert_eq!(
            p.event,
            LogEvent::PlayerJoined {
                display_name: "Alice".into(),
                user_id: Some("usr_11111111-2222-3333-4444-555566667777".into())
            }
        );
    }

    #[test]
    fn parses_player_joined_without_user_id_legacy() {
        let line = "2026.05.09 21:50:00 Log        -  [Behaviour] OnPlayerJoined Bob";
        let p = parse_line(line).unwrap();
        assert_eq!(
            p.event,
            LogEvent::PlayerJoined {
                display_name: "Bob".into(),
                user_id: None
            }
        );
    }

    #[test]
    fn parses_player_left() {
        let line = "2026.05.09 22:00:00 Log        -  [Behaviour] OnPlayerLeft Alice (usr_11111111-2222-3333-4444-555566667777)";
        let p = parse_line(line).unwrap();
        assert_eq!(
            p.event,
            LogEvent::PlayerLeft {
                display_name: "Alice".into(),
                user_id: Some("usr_11111111-2222-3333-4444-555566667777".into())
            }
        );
    }

    #[test]
    fn parses_notification_invite() {
        let line = "2026.05.09 22:30:00 Log        -  Received Notification: <Notification from username:Alice, sender user id:usr_xxx ... type:invite>";
        let p = parse_line(line).unwrap();
        assert_eq!(
            p.event,
            LogEvent::Notification {
                sender: "Alice".into(),
                ntype: "invite".into()
            }
        );
    }

    #[test]
    fn parses_notification_request_invite() {
        let line = "2026.05.09 22:31:00 Log        -  Received Notification: <Notification from username:Bob, ... type:requestInvite>";
        let p = parse_line(line).unwrap();
        assert_eq!(
            p.event,
            LogEvent::Notification {
                sender: "Bob".into(),
                ntype: "requestInvite".into()
            }
        );
    }

    #[test]
    fn parses_notification_boop() {
        let line = "2026.05.09 22:32:00 Log        -  Received Notification: <Notification from username:Charlie, ... type:boop>";
        let p = parse_line(line).unwrap();
        assert_eq!(
            p.event,
            LogEvent::Notification {
                sender: "Charlie".into(),
                ntype: "boop".into()
            }
        );
    }

    #[test]
    fn parses_video_url() {
        let line = "2026.05.09 22:40:00 Log        -  [Video Playback] AVProVideoMain Attempting to resolve URL 'https://www.youtube.com/watch?v=dQw4w9WgXcQ' from player";
        let p = parse_line(line).unwrap();
        assert_eq!(
            p.event,
            LogEvent::VideoUrl {
                url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into()
            }
        );
    }

    #[test]
    fn unknown_line_with_timestamp_yields_unparsable() {
        let line =
            "2026.05.09 21:00:00 Log        -  Some unknown framework chatter that does not match";
        let p = parse_line(line).unwrap();
        assert!(matches!(p.event, LogEvent::UnparsableLine { .. }));
    }

    #[test]
    fn line_without_timestamp_returns_none() {
        // ヘッダ行などタイムスタンプなしは skip 対象。
        assert!(parse_line("VRChat Build 2026.5.9").is_none());
        assert!(parse_line("").is_none());
        assert!(parse_line("\r\n").is_none());
    }

    #[test]
    fn handles_crlf_endings() {
        let line = "2026.05.09 21:43:56 Log        -  [Behaviour] Entering Room: HomeWorld\r\n";
        let p = parse_line(line).unwrap();
        assert_eq!(
            p.event,
            LogEvent::RoomEntering {
                world_name: "HomeWorld".into()
            }
        );
    }

    #[test]
    fn world_name_with_special_chars_preserved() {
        let line = "2026.05.09 21:43:56 Log        -  [Behaviour] Entering Room: 私のお気に入り! [v2.1] 🎮";
        let p = parse_line(line).unwrap();
        assert_eq!(
            p.event,
            LogEvent::RoomEntering {
                world_name: "私のお気に入り! [v2.1] 🎮".into()
            }
        );
    }

    #[test]
    fn type_tag_returns_static_kind() {
        let cases = [
            (
                LogEvent::RoomEntering {
                    world_name: "x".into(),
                },
                "RoomEntering",
            ),
            (
                LogEvent::RoomJoining {
                    world_id: "w".into(),
                    instance_id: "i".into(),
                },
                "RoomJoining",
            ),
            (
                LogEvent::UnparsableLine { reason: "x".into() },
                "UnparsableLine",
            ),
        ];
        for (e, expected) in cases {
            assert_eq!(e.type_tag(), expected);
        }
    }
}
