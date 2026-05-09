//! ユーザー設定。`settings.json` を atomic write し、parse 失敗時はバックアップを残す。
//!
//! # 設計上の不変条件
//!
//! - **起動時自動 write 禁止**: load で missing/corrupt を検出しても、`settings.json` は
//!   書き換えない。in-memory には default を入れて返す。明示操作 (例: autostart toggle、
//!   フォーム保存) でのみ write する。これにより、誤起動でユーザー設定を失わない。
//! - **atomic write**: 同一ディレクトリの一時ファイルに書き出してから rename で置換。
//!   並行 save が来ても settings_writer actor が直列化する。
//! - **corrupt 検出時**: `.corrupt-{ts}.bak` にバックアップを作成し、UI に warn event を
//!   出す前提。原本は触らないので、ユーザーが手動で復旧可能。

pub mod store;
pub mod writer;

pub use store::{LoadOutcome, Settings, SettingsLoadError};
pub use writer::{SettingsCommand, SettingsHandle, SettingsWriter};
