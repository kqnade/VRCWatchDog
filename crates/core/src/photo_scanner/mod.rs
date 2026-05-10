//! Phase 6: VRChat の screenshot を watch して [`crate::db::repo::photo_records`] に
//! 投入する pipeline。
//!
//! 現状は filename parser のみ landed。続く commit で WorldVisitMatcher / Actor /
//! bootstrap wiring を順に積む。
//!
//! 設計方針 (plan §3):
//! - notify-debouncer-mini で 500ms debounce
//! - .tmp 拡張子は skip (VRChat の write-in-flight)
//! - File::open の Windows 排他失敗は 100ms × 30 retry
//! - WorldVisitMatcher は world_visits を joined_utc で sort 済み配列にして binary search
//! - 写真原本の path 保護は plan §7 (asset:// で公開しない、open_photo command 経由のみ)

pub mod filename;
pub mod visit_matcher;

pub use filename::{parse_vrchat_filename, VRChatPhotoMeta};
pub use visit_matcher::{match_photo_to_visit, WorldVisitTimeRange};
