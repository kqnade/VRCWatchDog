//! Phase 6.3 thumb_writer: photo_records から thumb_sha=NULL の row を拾って、
//! `%LOCALAPPDATA%/VRCWatchDog/cache/thumbs/{sha}.webp` を生成し DB に sha を書き戻す。
//!
//! 設計方針 (plan §確定方針 + §7):
//! - サムネは webp 形式 (lossless、image 0.25 は lossy webp encoder を持たない)
//! - 配置先は `%LOCALAPPDATA%/VRCWatchDog/cache/thumbs/{sha}.webp`
//! - frontend は `tauri.conf.json` の `assetProtocol.scope` で
//!   `$LOCALAPPDATA/VRCWatchDog/cache/thumbs/**` だけが asset:// で配信される
//! - 写真原本は asset:// に乗せない (open_photo Rust command 経由のみ)
//!
//! 6.3.2 では render (= 純粋に画像 1 件を webp に変換する関数) を landing。
//! 6.3.3 で worker actor、6.3.4 で bootstrap wire。

pub mod actor;
pub mod render;

pub use actor::{BatchOutcome, ThumbWriterActor, ThumbWriterConfig};
pub use render::{render_thumb_to_webp, ThumbRenderError};
