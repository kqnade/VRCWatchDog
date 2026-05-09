//! データベース接続・マイグレーション・repository。
//!
//! repository 関数群は `&mut Transaction<'_, Sqlite>` を受け取り、tx 境界は
//! 呼び出し側で制御する設計。Phase 4a 以降で write actor から束ねて利用する。

mod connection;
pub mod repo;

pub use connection::{open, Pool};
