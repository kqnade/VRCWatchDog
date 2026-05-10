//! VRCWatchDog core: VRChat ログ監視・解析・永続化の中核ロジック。
//!
//! UI 非依存。Tauri アプリ (`crates/app`) や CLI examples から利用される。

pub mod db;
pub mod error;
pub mod health;
pub mod ipc;
pub mod log_parser;
pub mod log_watcher;
pub mod photo;
pub mod photo_scanner;
pub mod process_monitor;
pub mod projector;
pub mod settings;
pub mod thumb_writer;
pub mod time;

pub use error::{Error, Result};
