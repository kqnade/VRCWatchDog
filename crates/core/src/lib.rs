//! VRCWatchDog core: VRChat ログ監視・解析・永続化の中核ロジック。
//!
//! UI 非依存。Tauri アプリ (`crates/app`) や CLI examples から利用される。

pub mod db;
pub mod error;
pub mod log_parser;
pub mod log_watcher;
pub mod projector;
pub mod time;

pub use error::{Error, Result};
