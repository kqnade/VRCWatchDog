//! VRChat ログから検出した動画 URL の title / thumbnail を補完する補助サービス
//! (Phase D: VRCTimeline の VideoInfoService に相当)。
//!
//! - `normalize` : URL を比較・キャッシュキー用に正規化する。tracking parameter 等で
//!   同じ動画が別 URL で検出されても、negative cache や result の dedupe が
//!   利くようにする。
//! - `oembed`    : noembed.com を叩いて title + thumbnail_url を取得する。
//! - `actor`     : poll loop。`title=NULL` の video_records を順次処理し、成功すれば
//!   metadata + thumbnail を更新、失敗すれば failed_video_lookups に TTL 付きで記録。

pub mod actor;
pub mod normalize;
pub mod oembed;

pub use actor::{VideoInfoActor, VideoInfoConfig};
pub use normalize::{normalize_url, NormalizationVersion, NORMALIZATION_VERSION};
pub use oembed::{fetch_oembed, OembedClient, OembedInfo};
