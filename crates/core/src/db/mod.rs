//! データベース接続・マイグレーション・write actor。
//!
//! 全 write 操作は [`write_actor`] を経由してシリアル化する
//! (WAL モードでも SQLite は single writer のため、busy エラー回避目的)。

mod connection;

pub use connection::{open, Pool};
