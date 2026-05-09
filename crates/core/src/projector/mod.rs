//! `raw_log_events` → domain projection。
//!
//! 各 raw event を 1 件処理して該当 domain table へ反映する。
//!
//! 中核ルール:
//! - ledger (`projected_raw_events`) で idempotency を担保
//!   - 1 raw event = 1 ledger row。Pending → Done | Skipped | FailedRecorded
//! - `world_visits.resolution_state` で `RoomEntering` / `RoomJoining` の到着順を吸収
//!   - `Pending` → `Resolved` (Joining 受信)
//!   - `Pending` → `MissingJoin` (次の RoomEntering が先)
//!   - `Resolved` + same Joining → idempotent no-op
//!   - `Resolved` + different Joining → `Conflict`
//! - 順序は `(log_sequence_key, generation, byte_offset)` (reconcile が古い file を
//!   後から拾った場合でも時系列を維持)

mod project;

pub use project::{project_batch, project_one, ProjectorBatchResult, ProjectorOutcome};
