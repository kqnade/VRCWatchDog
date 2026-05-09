//! VRCWatchDog core: VRChat ログ監視・解析・永続化の中核ロジック。
//!
//! UI 非依存。Tauri アプリ (`crates/app`) や CLI examples から利用される。

pub mod error;

pub use error::{Error, Result};
