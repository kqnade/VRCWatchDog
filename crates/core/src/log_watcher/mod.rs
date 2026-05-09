//! ログ監視 actor。
//!
//! Phase 4a の中核。`FsEventSource` 経由で notify イベントを受け取り、
//! [`FileIdentity`] でファイル世代を追跡しつつ、
//! [`crate::db::repo::raw_log::insert_batch_with_ledger`] で raw + ledger を
//! 同一 tx 永続化する。

mod actor;
mod file_identity;
mod fs_event;
mod line_buffer;
mod notify_source;

pub use actor::{
    FsProbe, HandleOutcome, LogWatcherActor, ProbedFile, ReconcileOutcome, WatcherConfig,
};
pub use notify_source::{NotifyEventSource, RealFsProbe};

pub use file_identity::{
    compute_file_identity_hash, detect_bump, CurrentFileState, FileIdentity, GenerationBumpReason,
    PrevFileState,
};
pub use fs_event::{FakeFsEventSource, FsEvent, FsEventSource};
pub use line_buffer::LineBuffer;
