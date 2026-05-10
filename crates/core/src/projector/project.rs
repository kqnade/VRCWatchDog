use std::collections::HashMap;

use sqlx::{Sqlite, Transaction};
use tracing::warn;

use crate::db::repo::{
    notification_records, player_sessions, projected_raw_events, self_player_records,
    video_records, world_visits,
};
use crate::db::Pool;
use crate::ipc::events::LiveLogEvent;
use crate::log_parser::LogEvent;
use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectorOutcome {
    Done,
    Skipped,
    FailedRecorded(String),
}

#[derive(Debug, Clone, Default)]
pub struct ProjectorBatchResult {
    pub processed: usize,
    pub done: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Done となった raw イベントを UI 向け [`LiveLogEvent`] に整形した列。
    /// 永続化済みの commit 後に caller が `app.emit(LIVE_LOG_EVENT, ev)` で
    /// frontend に流す想定 (Phase B)。
    pub events: Vec<LiveLogEvent>,
}

/// raw payload を UI 向け [`LiveLogEvent`] に変換する。Done になった raw のみ
/// project_batch から呼び出される。`UserAuthenticated` / `UnparsableLine` は
/// UI ストリームに乗せたくないので None を返す。
fn to_live_event(event: &LogEvent, naive_local: chrono::NaiveDateTime) -> Option<LiveLogEvent> {
    let nl = naive_local.format("%Y-%m-%d %H:%M:%S").to_string();
    match event {
        LogEvent::RoomEntering { world_name } => Some(LiveLogEvent::WorldEntering {
            naive_local: nl,
            world_name: world_name.clone(),
        }),
        LogEvent::RoomJoining {
            world_id,
            instance_id,
        } => Some(LiveLogEvent::WorldJoining {
            naive_local: nl,
            world_id: world_id.clone(),
            instance_id: instance_id.clone(),
        }),
        LogEvent::PlayerJoined {
            display_name,
            user_id,
        } => Some(LiveLogEvent::PlayerJoined {
            naive_local: nl,
            display_name: display_name.clone(),
            user_id: user_id.clone(),
        }),
        LogEvent::PlayerLeft {
            display_name,
            user_id,
        } => Some(LiveLogEvent::PlayerLeft {
            naive_local: nl,
            display_name: display_name.clone(),
            user_id: user_id.clone(),
        }),
        LogEvent::Notification { sender, ntype } => Some(LiveLogEvent::Notification {
            naive_local: nl,
            sender: sender.clone(),
            ntype: ntype.clone(),
        }),
        LogEvent::VideoUrl { url } => Some(LiveLogEvent::VideoUrl {
            naive_local: nl,
            url: url.clone(),
        }),
        // UI には流さない (内部ステート遷移なので意味が薄い)
        LogEvent::UserAuthenticated { .. } | LogEvent::UnparsableLine { .. } => None,
    }
}

/// raw event を 1 件 projection する。tx 境界は呼び出し側が制御。
///
/// 失敗時は ledger に `FailedRecorded` を立てて Ok を返す。`Err` を返すのは
/// SQL 層で復旧不能なエラーが起きた場合だけ。
pub async fn project_one(
    tx: &mut Transaction<'_, Sqlite>,
    raw: &projected_raw_events::PendingRaw,
) -> Result<ProjectorOutcome> {
    let event: LogEvent = match serde_json::from_str(&raw.payload_json) {
        Ok(e) => e,
        Err(e) => {
            let msg = format!("payload deserialize: {e}");
            projected_raw_events::mark_failed(tx, raw.raw_event_id, &msg).await?;
            return Ok(ProjectorOutcome::FailedRecorded(msg));
        }
    };
    let Some(naive_local) = raw.naive_local else {
        projected_raw_events::mark_skipped(tx, raw.raw_event_id, "no naive_local").await?;
        return Ok(ProjectorOutcome::Skipped);
    };
    let ctx = world_visits::TimeContext {
        tz_id: &raw.tz_id,
        tz_source: &raw.tz_source,
        resolution_confidence: "High",
    };

    let outcome = match event {
        LogEvent::RoomEntering { world_name } => {
            // 既存 active があれば finalize_for_next_entering を先に走らせる
            if let Some(active) = world_visits::fetch_active(tx).await? {
                world_visits::finalize_for_next_entering(tx, active.id, naive_local, ctx, None)
                    .await?;
            }
            world_visits::insert_pending(tx, raw.raw_event_id, &world_name, naive_local, ctx, None)
                .await?;
            ProjectorOutcome::Done
        }
        LogEvent::RoomJoining {
            world_id,
            instance_id,
        } => match world_visits::fetch_active(tx).await? {
            Some(active) if active.resolution_state == "Pending" => {
                world_visits::resolve_pending_with_world(tx, &world_id, &instance_id).await?;
                ProjectorOutcome::Done
            }
            Some(active) => {
                let outcome = world_visits::handle_late_or_repeat_joining(
                    tx,
                    active.id,
                    &world_id,
                    &instance_id,
                )
                .await?;
                if outcome == world_visits::JoiningOutcome::Conflict {
                    warn!(
                        visit_id = active.id,
                        world_id, instance_id, "joining conflict detected"
                    );
                }
                ProjectorOutcome::Done
            }
            None => {
                projected_raw_events::mark_skipped(
                    tx,
                    raw.raw_event_id,
                    "joining without active visit",
                )
                .await?;
                return Ok(ProjectorOutcome::Skipped);
            }
        },
        LogEvent::PlayerJoined {
            display_name,
            user_id,
        } => match world_visits::fetch_active(tx).await? {
            Some(active) => {
                player_sessions::insert_join(
                    tx,
                    raw.raw_event_id,
                    active.id,
                    &display_name,
                    user_id.as_deref(),
                    naive_local,
                    ctx,
                )
                .await?;
                ProjectorOutcome::Done
            }
            None => {
                projected_raw_events::mark_skipped(
                    tx,
                    raw.raw_event_id,
                    "player_joined without active visit",
                )
                .await?;
                return Ok(ProjectorOutcome::Skipped);
            }
        },
        LogEvent::PlayerLeft {
            display_name,
            user_id,
        } => {
            if let Some(active) = world_visits::fetch_active(tx).await? {
                player_sessions::set_left(
                    tx,
                    active.id,
                    &display_name,
                    user_id.as_deref(),
                    naive_local,
                    ctx,
                )
                .await?;
            }
            ProjectorOutcome::Done
        }
        LogEvent::Notification { sender, ntype } => {
            let active_id = world_visits::fetch_active(tx).await?.map(|v| v.id);
            notification_records::insert(
                tx,
                raw.raw_event_id,
                active_id,
                &sender,
                &ntype,
                naive_local,
                ctx,
            )
            .await?;
            ProjectorOutcome::Done
        }
        LogEvent::VideoUrl { url } => {
            let active_id = world_visits::fetch_active(tx).await?.map(|v| v.id);
            video_records::insert(tx, raw.raw_event_id, active_id, &url, naive_local, ctx).await?;
            ProjectorOutcome::Done
        }
        LogEvent::UserAuthenticated { display_name } => {
            // Phase G: 自分自身の認証ログを self_player_records に projection。
            // 同 raw_id で UNIQUE なので重複なし。display_name のみ抽出 (user_id は VRChat
            // ログ形式では同行に出ないため、UI 用途では display_name で十分)。
            self_player_records::insert(tx, raw.raw_event_id, &display_name, naive_local, ctx)
                .await?;
            ProjectorOutcome::Done
        }
        LogEvent::UnparsableLine { .. } => {
            projected_raw_events::mark_skipped(tx, raw.raw_event_id, "unparsable").await?;
            return Ok(ProjectorOutcome::Skipped);
        }
    };

    if matches!(outcome, ProjectorOutcome::Done) {
        projected_raw_events::mark_done(tx, raw.raw_event_id).await?;
    }
    Ok(outcome)
}

/// `Pending` 状態の raw を batch_size 件取り出して projection する。
/// `processed_log_files.last_projected_raw_event_id` を batch 内の最大値で前進。
pub async fn project_batch(pool: &Pool, batch_size: i64) -> Result<ProjectorBatchResult> {
    let mut tx = pool.begin().await?;
    let pending = projected_raw_events::fetch_pending_batch(&mut tx, batch_size).await?;
    if pending.is_empty() {
        tx.commit().await?;
        return Ok(ProjectorBatchResult::default());
    }

    let mut result = ProjectorBatchResult {
        processed: pending.len(),
        ..Default::default()
    };
    for raw in &pending {
        match project_one(&mut tx, raw).await? {
            ProjectorOutcome::Done => {
                result.done += 1;
                // UI 向けに LiveLogEvent を組み立てる (Phase B)。serde_json::from_str は
                // project_one 内で 1 度成功しているはずだが、ownership を回さないために
                // ここでもう 1 度パースする。payload は数百 byte 程度なので無視できる。
                if let (Ok(ev), Some(nl)) = (
                    serde_json::from_str::<LogEvent>(&raw.payload_json),
                    raw.naive_local,
                ) {
                    if let Some(live) = to_live_event(&ev, nl) {
                        result.events.push(live);
                    }
                }
            }
            ProjectorOutcome::Skipped => result.skipped += 1,
            ProjectorOutcome::FailedRecorded(_) => result.failed += 1,
        }
    }

    // checkpoint: 各 file ごとに max(raw_event_id) を進める
    let mut max_per_file: HashMap<i64, i64> = HashMap::new();
    for raw in &pending {
        let entry = max_per_file
            .entry(raw.processed_log_file_id)
            .or_insert(raw.raw_event_id);
        if *entry < raw.raw_event_id {
            *entry = raw.raw_event_id;
        }
    }
    for (pf_id, raw_id) in max_per_file {
        projected_raw_events::set_last_projected(&mut tx, pf_id, raw_id).await?;
    }
    tx.commit().await?;
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::open;
    use crate::db::repo::processed_log_files::{self, ProcessedLogFileInput};
    use crate::db::repo::raw_log::{self, RawEventInput};
    use chrono::{NaiveDate, NaiveDateTime, Utc};
    use sqlx::Row;
    use tempfile::tempdir;

    async fn setup() -> (sqlx::SqlitePool, tempfile::TempDir, i64) {
        let dir = tempdir().unwrap();
        let pool = open(&dir.path().join("test.db")).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let pf_id = processed_log_files::upsert(
            &mut tx,
            &ProcessedLogFileInput {
                file_identity_hash: "h1".into(),
                log_sequence_key: "s".into(),
                volume_serial: 0,
                file_id_high: 0,
                file_id_low: 0,
                generation: 0,
                creation_time: 0,
                first_kb_hash: "k".into(),
                file_name: "a.txt".into(),
                file_size: 0,
                mtime: Utc::now(),
                tz_id: "Asia/Tokyo".into(),
                tz_source: "CapturedRealtime".into(),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        (pool, dir, pf_id)
    }

    fn nd(h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 5, 9)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    async fn add_raw(
        pool: &sqlx::SqlitePool,
        pf_id: i64,
        offset: i64,
        event: LogEvent,
        ts: NaiveDateTime,
    ) {
        let mut tx = pool.begin().await.unwrap();
        raw_log::insert_batch_with_ledger(
            &mut tx,
            &[RawEventInput {
                processed_log_file_id: pf_id,
                byte_offset: offset,
                event,
                naive_local: Some(ts),
            }],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    /// 不変条件: RoomEntering → RoomJoining の順で project すると visit は Resolved に。
    #[tokio::test]
    async fn entering_then_joining_resolves_visit() {
        let (pool, _dir, pf_id) = setup().await;
        add_raw(
            &pool,
            pf_id,
            0,
            LogEvent::RoomEntering {
                world_name: "Alpha".into(),
            },
            nd(21, 0),
        )
        .await;
        add_raw(
            &pool,
            pf_id,
            100,
            LogEvent::RoomJoining {
                world_id: "wrld_x".into(),
                instance_id: "12345".into(),
            },
            nd(21, 0),
        )
        .await;

        let r = project_batch(&pool, 100).await.unwrap();
        assert_eq!(r.processed, 2);
        assert_eq!(r.done, 2);

        let row: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT resolution_state, world_id, instance_id FROM world_visits LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "Resolved");
        assert_eq!(row.1.as_deref(), Some("wrld_x"));
        assert_eq!(row.2.as_deref(), Some("12345"));
    }

    /// 不変条件: 連続 RoomEntering で前 visit が MissingJoin に遷移。
    #[tokio::test]
    async fn back_to_back_entering_promotes_pending_to_missing_join() {
        let (pool, _dir, pf_id) = setup().await;
        add_raw(
            &pool,
            pf_id,
            0,
            LogEvent::RoomEntering {
                world_name: "A".into(),
            },
            nd(21, 0),
        )
        .await;
        add_raw(
            &pool,
            pf_id,
            100,
            LogEvent::RoomEntering {
                world_name: "B".into(),
            },
            nd(22, 0),
        )
        .await;
        project_batch(&pool, 100).await.unwrap();

        let states: Vec<(String, String)> =
            sqlx::query_as("SELECT world_name, resolution_state FROM world_visits ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0], ("A".into(), "MissingJoin".into()));
        assert_eq!(states[1], ("B".into(), "Pending".into()));
    }

    /// 不変条件: Player Join/Leave が active visit に紐づく。
    #[tokio::test]
    async fn player_join_and_leave_attached_to_active_visit() {
        let (pool, _dir, pf_id) = setup().await;
        add_raw(
            &pool,
            pf_id,
            0,
            LogEvent::RoomEntering {
                world_name: "A".into(),
            },
            nd(21, 0),
        )
        .await;
        add_raw(
            &pool,
            pf_id,
            100,
            LogEvent::RoomJoining {
                world_id: "wrld_x".into(),
                instance_id: "12345".into(),
            },
            nd(21, 0),
        )
        .await;
        add_raw(
            &pool,
            pf_id,
            200,
            LogEvent::PlayerJoined {
                display_name: "Alice".into(),
                user_id: Some("usr_a".into()),
            },
            nd(21, 5),
        )
        .await;
        add_raw(
            &pool,
            pf_id,
            300,
            LogEvent::PlayerLeft {
                display_name: "Alice".into(),
                user_id: Some("usr_a".into()),
            },
            nd(21, 30),
        )
        .await;
        project_batch(&pool, 100).await.unwrap();

        let row: (String, Option<String>, Option<String>) =
            sqlx::query_as("SELECT display_name, user_id, left_utc FROM player_sessions LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "Alice");
        assert_eq!(row.1.as_deref(), Some("usr_a"));
        assert!(row.2.is_some(), "left_utc must be filled");
    }

    /// 不変条件: Notification / VideoUrl が active visit に紐づく。
    #[tokio::test]
    async fn notification_and_video_attached_to_active_visit() {
        let (pool, _dir, pf_id) = setup().await;
        add_raw(
            &pool,
            pf_id,
            0,
            LogEvent::RoomEntering {
                world_name: "A".into(),
            },
            nd(21, 0),
        )
        .await;
        add_raw(
            &pool,
            pf_id,
            100,
            LogEvent::Notification {
                sender: "Bob".into(),
                ntype: "invite".into(),
            },
            nd(21, 5),
        )
        .await;
        add_raw(
            &pool,
            pf_id,
            200,
            LogEvent::VideoUrl {
                url: "https://youtu.be/x".into(),
            },
            nd(21, 10),
        )
        .await;
        project_batch(&pool, 100).await.unwrap();

        let n_row: (String, String, Option<i64>) = sqlx::query_as(
            "SELECT sender_name, notification_type, world_visit_id FROM notification_records",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n_row.0, "Bob");
        assert_eq!(n_row.1, "invite");
        assert!(n_row.2.is_some());

        let v_row: (String, Option<i64>) =
            sqlx::query_as("SELECT url, world_visit_id FROM video_records")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(v_row.0, "https://youtu.be/x");
        assert!(v_row.1.is_some());
    }

    /// 不変条件: project_batch を 2 回呼んでも domain 行は増えない (idempotency)。
    #[tokio::test]
    async fn project_batch_is_idempotent_when_re_run() {
        let (pool, _dir, pf_id) = setup().await;
        add_raw(
            &pool,
            pf_id,
            0,
            LogEvent::RoomEntering {
                world_name: "A".into(),
            },
            nd(21, 0),
        )
        .await;
        add_raw(
            &pool,
            pf_id,
            100,
            LogEvent::PlayerJoined {
                display_name: "Alice".into(),
                user_id: Some("usr_a".into()),
            },
            nd(21, 5),
        )
        .await;
        let r1 = project_batch(&pool, 100).await.unwrap();
        let r2 = project_batch(&pool, 100).await.unwrap();
        assert_eq!(r1.processed, 2);
        assert_eq!(r2.processed, 0, "second run finds no Pending raw");

        let visits: i64 = sqlx::query("SELECT COUNT(*) FROM world_visits")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        let sessions: i64 = sqlx::query("SELECT COUNT(*) FROM player_sessions")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(visits, 1);
        assert_eq!(sessions, 1);
    }

    /// Phase B: project_batch は Done になった raw について
    /// LiveLogEvent を `events` vec に積む。WorldEntering は emit、UserAuthenticated は除外。
    #[tokio::test]
    async fn project_batch_collects_live_events_for_done_outcomes_only() {
        let (pool, _dir, pf_id) = setup().await;
        // RoomEntering (Done) + UserAuthenticated (Done だが UI 対象外) + UnparsableLine (Skipped)
        add_raw(
            &pool,
            pf_id,
            0,
            LogEvent::RoomEntering {
                world_name: "Hub".into(),
            },
            nd(20, 0),
        )
        .await;
        add_raw(
            &pool,
            pf_id,
            100,
            LogEvent::UserAuthenticated {
                display_name: "kqnade".into(),
            },
            nd(20, 1),
        )
        .await;
        add_raw(
            &pool,
            pf_id,
            200,
            LogEvent::UnparsableLine { reason: "x".into() },
            nd(20, 2),
        )
        .await;

        let r = project_batch(&pool, 100).await.unwrap();

        // events は WorldEntering 1 件のみ (Done だが UserAuthenticated は除外、Unparsable は Skipped)
        assert_eq!(r.events.len(), 1);
        match &r.events[0] {
            LiveLogEvent::WorldEntering { world_name, .. } => {
                assert_eq!(world_name, "Hub");
            }
            other => panic!("expected WorldEntering, got {other:?}"),
        }
    }

    /// 不変条件: UnparsableLine は Skipped、UserAuthenticated は Done で
    /// self_player_records に 1 行 insert される (Phase G 以降)。
    #[tokio::test]
    async fn unparsable_skipped_and_user_authenticated_projected_to_self_player() {
        let (pool, _dir, pf_id) = setup().await;
        add_raw(
            &pool,
            pf_id,
            0,
            LogEvent::UnparsableLine { reason: "x".into() },
            nd(21, 0),
        )
        .await;
        add_raw(
            &pool,
            pf_id,
            100,
            LogEvent::UserAuthenticated {
                display_name: "kqnade".into(),
            },
            nd(21, 1),
        )
        .await;
        let r = project_batch(&pool, 100).await.unwrap();
        assert_eq!(r.processed, 2);
        assert_eq!(r.skipped, 1, "UnparsableLine のみ skipped");
        assert_eq!(r.done, 1, "UserAuthenticated は self_player に project");

        // world_visits は依然 0 件 (UserAuthenticated は visit を作らない)
        let visits: i64 = sqlx::query("SELECT COUNT(*) FROM world_visits")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(visits, 0);

        // self_player_records には 1 件 (display_name=kqnade)
        let self_count: i64 = sqlx::query("SELECT COUNT(*) FROM self_player_records")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(self_count, 1);
        let name: String =
            sqlx::query("SELECT display_name FROM self_player_records ORDER BY id DESC LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(name, "kqnade");

        // raw_event_id 順 (= insert 順) に取り出すと:
        // [0] = UnparsableLine  → Skipped
        // [1] = UserAuthenticated → Done (self_player_records に projection)
        let statuses: Vec<(String,)> =
            sqlx::query_as("SELECT status FROM projected_raw_events ORDER BY raw_event_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].0, "Skipped");
        assert_eq!(statuses[1].0, "Done");
    }
}
