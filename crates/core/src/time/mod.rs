//! 時刻・タイムゾーン関連ユーティリティ。
//!
//! VRChat ログのタイムスタンプはタイムゾーン情報を持たないため、
//! IANA タイムゾーンを別途与えて UTC に解決する必要がある。
//! DST の Ambiguous / Gap も明示的に扱う ([`Resolution`])。

mod duration;
mod resolve;

pub use duration::format_duration_hms;
pub use resolve::{resolve_local_to_utc, Resolution};
