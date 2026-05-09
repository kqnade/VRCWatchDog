//! テーブル別 repository 関数群。
//!
//! 各関数は `&mut Transaction<'_, Sqlite>` を取り、tx 境界は呼び出し側 (主に
//! [`super::write_actor`]) が制御する。これにより複数 repo 操作を 1 transaction に
//! 束ねる柔軟性と、テストで `tx.rollback()` で確実にクリーンに戻せる利点がある。

pub mod notification_records;
pub mod player_sessions;
pub mod processed_log_files;
pub mod projected_raw_events;
pub mod raw_log;
pub mod video_records;
pub mod world_visits;
