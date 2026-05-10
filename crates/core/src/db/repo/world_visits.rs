//! `world_visits` repository.
//!
//! Phase 4b 中核: VRChat ログの `Entering Room` / `Joining wrld_xxx:nonce` の
//! 到着順 (room name 先、world_id 後) を吸収するため、 `resolution_state` で
//! 5 状態遷移を表現する。
//!
//! - `Pending`: Entering Room 受信、 Joining 待ち
//! - `Resolved`: Joining 受信、 world_id/instance_id 確定
//! - `MissingJoin`: 次の Entering Room までに Joining が来ず確定失敗
//! - `ClosedWithoutJoin`: VRChat process exit が先 (Phase 7 で finalize)
//! - `Conflict`: 矛盾入力 (Resolved 中に別 Joining、 grace window 後の late Joining 等)

use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::{Sqlite, Transaction};

use crate::time::resolve_local_to_utc;
use crate::Result;

/// タイムゾーン解決のコンテキスト。projector が raw event 単位で都度供給する。
#[derive(Debug, Clone, Copy)]
pub struct TimeContext<'a> {
    pub tz_id: &'a str,
    pub tz_source: &'a str,
    pub resolution_confidence: &'a str, // "High" / "Medium" / "Low"
}

/// `fetch_active` の戻り値。projector が「現在のアクティブ visit」を判断するのに使う。
#[derive(Debug, Clone)]
pub struct ActiveVisit {
    pub id: i64,
    pub resolution_state: String,
    pub world_id: Option<String>,
    pub world_name: String,
    pub joined_utc: DateTime<Utc>,
}

fn parse_tz(tz_id: &str) -> Result<chrono_tz::Tz> {
    tz_id
        .parse::<chrono_tz::Tz>()
        .map_err(|_| crate::Error::Config(format!("invalid tz: {tz_id}")))
}

fn fmt_naive(n: NaiveDateTime) -> String {
    n.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// `RoomEntering` を受けて `Pending` の visit を新規作成する。
/// `source_raw_event_id` UNIQUE で idempotent: 既存があれば既存 id を返す。
pub async fn insert_pending(
    tx: &mut Transaction<'_, Sqlite>,
    raw_id: i64,
    world_name: &str,
    naive_local: NaiveDateTime,
    ctx: TimeContext<'_>,
    prev_utc: Option<DateTime<Utc>>,
) -> Result<i64> {
    // 既存チェック (idempotency)
    let existing: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM world_visits WHERE source_raw_event_id = ?1")
            .bind(raw_id)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some((id,)) = existing {
        return Ok(id);
    }

    let tz = parse_tz(ctx.tz_id)?;
    let (utc, offset, res) = resolve_local_to_utc(naive_local, &tz, prev_utc);
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO world_visits (
            source_raw_event_id, world_name,
            resolution_state,
            joined_naive_local, joined_utc,
            joined_tz_id, joined_offset_seconds,
            joined_resolution, joined_tz_source, joined_resolution_confidence
        ) VALUES (?1, ?2, 'Pending', ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        RETURNING id",
    )
    .bind(raw_id)
    .bind(world_name)
    .bind(fmt_naive(naive_local))
    .bind(utc.to_rfc3339())
    .bind(ctx.tz_id)
    .bind(offset)
    .bind(res.as_str())
    .bind(ctx.tz_source)
    .bind(ctx.resolution_confidence)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

/// 現在のアクティブ visit (= `Pending`/`Resolved` で `left_utc IS NULL`) を取得。
/// Joining wrld の宛先や、次の RoomEntering で finalize する対象を選ぶのに使う。
pub async fn fetch_active(tx: &mut Transaction<'_, Sqlite>) -> Result<Option<ActiveVisit>> {
    let row: Option<(i64, String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, resolution_state, world_id, world_name, joined_utc
         FROM world_visits
         WHERE resolution_state IN ('Pending', 'Resolved')
           AND left_utc IS NULL
         ORDER BY joined_utc DESC
         LIMIT 1",
    )
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(id, state, world_id, world_name, utc_str)| {
        let joined_utc = DateTime::parse_from_rfc3339(&utc_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        ActiveVisit {
            id,
            resolution_state: state,
            world_id,
            world_name,
            joined_utc,
        }
    }))
}

/// `Joining wrld_xxx:nonce` を受けて、 `Pending` の visit を `Resolved` に遷移させる。
///
/// 戻り値:
/// - `Some(id)`: 該当 Pending visit があり、 Resolved 化した
/// - `None`: 該当なし (RoomEntering より先に Joining だけが来た稀ケース)
pub async fn resolve_pending_with_world(
    tx: &mut Transaction<'_, Sqlite>,
    world_id: &str,
    instance_id: &str,
) -> Result<Option<i64>> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM world_visits
         WHERE resolution_state = 'Pending' AND left_utc IS NULL
         ORDER BY joined_utc DESC LIMIT 1",
    )
    .fetch_optional(&mut **tx)
    .await?;
    let Some((visit_id,)) = row else {
        return Ok(None);
    };

    sqlx::query(
        "UPDATE world_visits
         SET world_id = ?1, instance_id = ?2, resolution_state = 'Resolved'
         WHERE id = ?3",
    )
    .bind(world_id)
    .bind(instance_id)
    .bind(visit_id)
    .execute(&mut **tx)
    .await?;
    Ok(Some(visit_id))
}

/// 既存 visit を `Resolved` であっても新たな `Joining` で「上書き」する場合の処理。
/// 入力された `world_id`/`instance_id` が現在値と一致すれば idempotent (no-op)、
/// 異なれば `Conflict` 状態に遷移する。
pub async fn handle_late_or_repeat_joining(
    tx: &mut Transaction<'_, Sqlite>,
    visit_id: i64,
    world_id: &str,
    instance_id: &str,
) -> Result<JoiningOutcome> {
    let row: (Option<String>, Option<String>, String) = sqlx::query_as(
        "SELECT world_id, instance_id, resolution_state FROM world_visits WHERE id = ?1",
    )
    .bind(visit_id)
    .fetch_one(&mut **tx)
    .await?;
    let (existing_world_id, existing_instance_id, _state) = row;
    let same = existing_world_id.as_deref() == Some(world_id)
        && existing_instance_id.as_deref() == Some(instance_id);
    if same {
        return Ok(JoiningOutcome::IdempotentNoop);
    }

    let detail = serde_json::json!({
        "kind": "joining_mismatch",
        "existing": { "world_id": existing_world_id, "instance_id": existing_instance_id },
        "incoming": { "world_id": world_id, "instance_id": instance_id },
    })
    .to_string();
    sqlx::query(
        "UPDATE world_visits
         SET resolution_state = 'Conflict', conflict_detail = ?1
         WHERE id = ?2",
    )
    .bind(&detail)
    .bind(visit_id)
    .execute(&mut **tx)
    .await?;
    Ok(JoiningOutcome::Conflict)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoiningOutcome {
    IdempotentNoop,
    Conflict,
}

/// 次の `RoomEntering` を受けるとき、 既存 active visit を finalize する。
///
/// state 遷移:
/// - `Pending` → `MissingJoin` (Joining が来ないまま閉じる)
/// - `Resolved` → `Resolved` のまま、 `left_*` を埋めるだけ
/// - `Conflict` → そのまま、 `left_*` を埋めるだけ
pub async fn finalize_for_next_entering(
    tx: &mut Transaction<'_, Sqlite>,
    visit_id: i64,
    new_naive_local: NaiveDateTime,
    ctx: TimeContext<'_>,
    prev_utc: Option<DateTime<Utc>>,
) -> Result<()> {
    let row: (String,) = sqlx::query_as("SELECT resolution_state FROM world_visits WHERE id = ?1")
        .bind(visit_id)
        .fetch_one(&mut **tx)
        .await?;
    let new_state = if row.0 == "Pending" {
        "MissingJoin"
    } else {
        // Resolved / Conflict はそのまま
        return finalize_left_only(tx, visit_id, new_naive_local, ctx, prev_utc).await;
    };

    let tz = parse_tz(ctx.tz_id)?;
    let (utc, offset, res) = resolve_local_to_utc(new_naive_local, &tz, prev_utc);
    sqlx::query(
        "UPDATE world_visits
         SET resolution_state = ?1,
             left_naive_local = ?2, left_utc = ?3,
             left_tz_id = ?4, left_offset_seconds = ?5,
             left_resolution = ?6, left_tz_source = ?7, left_resolution_confidence = ?8
         WHERE id = ?9",
    )
    .bind(new_state)
    .bind(fmt_naive(new_naive_local))
    .bind(utc.to_rfc3339())
    .bind(ctx.tz_id)
    .bind(offset)
    .bind(res.as_str())
    .bind(ctx.tz_source)
    .bind(ctx.resolution_confidence)
    .bind(visit_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn finalize_left_only(
    tx: &mut Transaction<'_, Sqlite>,
    visit_id: i64,
    new_naive_local: NaiveDateTime,
    ctx: TimeContext<'_>,
    prev_utc: Option<DateTime<Utc>>,
) -> Result<()> {
    let tz = parse_tz(ctx.tz_id)?;
    let (utc, offset, res) = resolve_local_to_utc(new_naive_local, &tz, prev_utc);
    sqlx::query(
        "UPDATE world_visits
         SET left_naive_local = ?1, left_utc = ?2,
             left_tz_id = ?3, left_offset_seconds = ?4,
             left_resolution = ?5, left_tz_source = ?6, left_resolution_confidence = ?7
         WHERE id = ?8",
    )
    .bind(fmt_naive(new_naive_local))
    .bind(utc.to_rfc3339())
    .bind(ctx.tz_id)
    .bind(offset)
    .bind(res.as_str())
    .bind(ctx.tz_source)
    .bind(ctx.resolution_confidence)
    .bind(visit_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// `list_recent_with_photo_counts` の戻り値要素。activity_history 画面用。
///
/// `photo_count` は `photo_records` を `world_visit_id` で外部結合してカウントしたもの。
/// 0 件 (写真なし visit) でも row は返る (LEFT JOIN + COUNT)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisitWithCounts {
    pub id: i64,
    pub world_id: Option<String>,
    pub world_name: String,
    pub joined_utc: DateTime<Utc>,
    pub left_utc: Option<DateTime<Utc>>,
    pub resolution_state: String,
    pub photo_count: i64,
}

/// 直近の visit を `joined_utc DESC` で最大 `limit` 件返す。各 row には紐づく
/// `photo_records` の件数を `LEFT JOIN + COUNT` で同梱する。
///
/// `limit <= 0` は空 vec (他 repo helpers と揃えた防衛)。
pub async fn list_recent_with_photo_counts(
    tx: &mut Transaction<'_, Sqlite>,
    limit: i64,
) -> Result<Vec<VisitWithCounts>> {
    use sqlx::Row;
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT v.id AS id,
                v.world_id AS world_id,
                v.world_name AS world_name,
                v.joined_utc AS joined_utc,
                v.left_utc AS left_utc,
                v.resolution_state AS resolution_state,
                COUNT(pr.id) AS photo_count
         FROM world_visits v
         LEFT JOIN photo_records pr ON pr.world_visit_id = v.id
         GROUP BY v.id
         ORDER BY v.joined_utc DESC
         LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(VisitWithCounts {
            id: row.try_get("id")?,
            world_id: row.try_get("world_id")?,
            world_name: row.try_get("world_name")?,
            joined_utc: row.try_get("joined_utc")?,
            left_utc: row.try_get("left_utc")?,
            resolution_state: row.try_get("resolution_state")?,
            photo_count: row.try_get("photo_count")?,
        });
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::open;
    use crate::db::repo::processed_log_files::{self, ProcessedLogFileInput};
    use crate::db::repo::raw_log::{self, RawEventInput};
    use crate::log_parser::LogEvent;
    use chrono::NaiveDate;
    use sqlx::Row;
    use tempfile::tempdir;

    async fn fresh_pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = open(&dir.path().join("test.db")).await.unwrap();
        (pool, dir)
    }

    fn ctx() -> TimeContext<'static> {
        TimeContext {
            tz_id: "Asia/Tokyo",
            tz_source: "CapturedRealtime",
            resolution_confidence: "High",
        }
    }

    fn nd(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    async fn seed_pf(tx: &mut Transaction<'_, Sqlite>) -> i64 {
        processed_log_files::upsert(
            tx,
            &ProcessedLogFileInput {
                file_identity_hash: "h1".into(),
                log_sequence_key: "2026-05-09_00-00-00".into(),
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
        .unwrap()
    }

    async fn seed_raw(
        tx: &mut Transaction<'_, Sqlite>,
        pf_id: i64,
        offset: i64,
        event: LogEvent,
    ) -> i64 {
        raw_log::insert_batch_with_ledger(
            tx,
            &[RawEventInput {
                processed_log_file_id: pf_id,
                byte_offset: offset,
                event,
                naive_local: Some(nd(2026, 5, 9, 21, 0, 0)),
            }],
        )
        .await
        .unwrap()[0]
    }

    /// 不変条件: RoomEntering で Pending visit を作成、 source_raw_event_id で idempotent。
    #[tokio::test]
    async fn insert_pending_creates_visit_and_is_idempotent() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf_id = seed_pf(&mut tx).await;
        let raw_id = seed_raw(
            &mut tx,
            pf_id,
            0,
            LogEvent::RoomEntering {
                world_name: "Alpha".into(),
            },
        )
        .await;

        let v1 = insert_pending(
            &mut tx,
            raw_id,
            "Alpha",
            nd(2026, 5, 9, 21, 0, 0),
            ctx(),
            None,
        )
        .await
        .unwrap();
        let v2 = insert_pending(
            &mut tx,
            raw_id,
            "Alpha",
            nd(2026, 5, 9, 21, 0, 0),
            ctx(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(v1, v2);

        let count: i64 = sqlx::query("SELECT COUNT(*) FROM world_visits")
            .fetch_one(&mut *tx)
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 1);

        let state: String = sqlx::query("SELECT resolution_state FROM world_visits WHERE id = ?1")
            .bind(v1)
            .fetch_one(&mut *tx)
            .await
            .unwrap()
            .get(0);
        assert_eq!(state, "Pending");
    }

    /// 不変条件: Pending visit があるとき、 resolve_pending_with_world で Resolved に遷移。
    #[tokio::test]
    async fn resolve_pending_promotes_to_resolved() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf_id = seed_pf(&mut tx).await;
        let raw_id = seed_raw(
            &mut tx,
            pf_id,
            0,
            LogEvent::RoomEntering {
                world_name: "Alpha".into(),
            },
        )
        .await;
        let v1 = insert_pending(
            &mut tx,
            raw_id,
            "Alpha",
            nd(2026, 5, 9, 21, 0, 0),
            ctx(),
            None,
        )
        .await
        .unwrap();
        let resolved = resolve_pending_with_world(&mut tx, "wrld_x", "12345~public")
            .await
            .unwrap();
        assert_eq!(resolved, Some(v1));

        let row: (Option<String>, Option<String>, String) = sqlx::query_as(
            "SELECT world_id, instance_id, resolution_state FROM world_visits WHERE id = ?1",
        )
        .bind(v1)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(row.0.as_deref(), Some("wrld_x"));
        assert_eq!(row.1.as_deref(), Some("12345~public"));
        assert_eq!(row.2, "Resolved");
    }

    /// 不変条件: Pending visit が無いとき、 resolve_pending は None を返す。
    #[tokio::test]
    async fn resolve_pending_returns_none_when_no_pending() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let r = resolve_pending_with_world(&mut tx, "wrld_x", "12345")
            .await
            .unwrap();
        assert!(r.is_none());
    }

    /// 不変条件: 連続 Entering Room → 前 visit が Pending なら MissingJoin に遷移。
    #[tokio::test]
    async fn next_entering_promotes_pending_to_missing_join() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf_id = seed_pf(&mut tx).await;
        let raw1 = seed_raw(
            &mut tx,
            pf_id,
            0,
            LogEvent::RoomEntering {
                world_name: "A".into(),
            },
        )
        .await;
        let v1 = insert_pending(&mut tx, raw1, "A", nd(2026, 5, 9, 21, 0, 0), ctx(), None)
            .await
            .unwrap();
        // 次の Entering Room (Joining なし) → MissingJoin に遷移
        finalize_for_next_entering(&mut tx, v1, nd(2026, 5, 9, 21, 30, 0), ctx(), None)
            .await
            .unwrap();
        let state: String = sqlx::query("SELECT resolution_state FROM world_visits WHERE id = ?1")
            .bind(v1)
            .fetch_one(&mut *tx)
            .await
            .unwrap()
            .get(0);
        assert_eq!(state, "MissingJoin");
        let left_utc: Option<String> =
            sqlx::query("SELECT left_utc FROM world_visits WHERE id = ?1")
                .bind(v1)
                .fetch_one(&mut *tx)
                .await
                .unwrap()
                .get(0);
        assert!(left_utc.is_some(), "left_utc must be filled");
    }

    /// 不変条件: Resolved 状態の visit に対する 同じ Joining は idempotent (no-op)。
    /// 異なる Joining は Conflict 遷移。
    #[tokio::test]
    async fn handle_late_or_repeat_joining_classifies_correctly() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf_id = seed_pf(&mut tx).await;
        let raw_id = seed_raw(
            &mut tx,
            pf_id,
            0,
            LogEvent::RoomEntering {
                world_name: "A".into(),
            },
        )
        .await;
        let v1 = insert_pending(&mut tx, raw_id, "A", nd(2026, 5, 9, 21, 0, 0), ctx(), None)
            .await
            .unwrap();
        resolve_pending_with_world(&mut tx, "wrld_x", "12345")
            .await
            .unwrap();

        // 同じ Joining → idempotent
        let outcome = handle_late_or_repeat_joining(&mut tx, v1, "wrld_x", "12345")
            .await
            .unwrap();
        assert_eq!(outcome, JoiningOutcome::IdempotentNoop);
        let state: String = sqlx::query("SELECT resolution_state FROM world_visits WHERE id = ?1")
            .bind(v1)
            .fetch_one(&mut *tx)
            .await
            .unwrap()
            .get(0);
        assert_eq!(state, "Resolved");

        // 異なる Joining → Conflict
        let outcome = handle_late_or_repeat_joining(&mut tx, v1, "wrld_y", "67890")
            .await
            .unwrap();
        assert_eq!(outcome, JoiningOutcome::Conflict);
        let row: (String, Option<String>) = sqlx::query_as(
            "SELECT resolution_state, conflict_detail FROM world_visits WHERE id = ?1",
        )
        .bind(v1)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(row.0, "Conflict");
        assert!(row.1.is_some());
    }

    /// 不変条件: fetch_active は最新のアクティブ visit (Pending/Resolved かつ left_utc IS NULL)
    /// を joined_utc 降順で 1 件返す。
    #[tokio::test]
    async fn fetch_active_picks_newest_unfinished_visit() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf_id = seed_pf(&mut tx).await;
        let raw1 = seed_raw(
            &mut tx,
            pf_id,
            0,
            LogEvent::RoomEntering {
                world_name: "A".into(),
            },
        )
        .await;
        let raw2 = seed_raw(
            &mut tx,
            pf_id,
            100,
            LogEvent::RoomEntering {
                world_name: "B".into(),
            },
        )
        .await;
        let v1 = insert_pending(&mut tx, raw1, "A", nd(2026, 5, 9, 20, 0, 0), ctx(), None)
            .await
            .unwrap();
        // v1 を MissingJoin で閉じてから v2 を作る
        finalize_for_next_entering(&mut tx, v1, nd(2026, 5, 9, 21, 0, 0), ctx(), None)
            .await
            .unwrap();
        let v2 = insert_pending(&mut tx, raw2, "B", nd(2026, 5, 9, 21, 0, 0), ctx(), None)
            .await
            .unwrap();
        let active = fetch_active(&mut tx).await.unwrap().unwrap();
        assert_eq!(active.id, v2);
        assert_eq!(active.world_name, "B");
        assert_eq!(active.resolution_state, "Pending");
    }

    // -- list_recent_with_photo_counts (Phase 6.4) --------------------------------

    /// photo_records に visit_id 付きで N 件 attach する helper。
    async fn attach_n_photos(pool: &sqlx::SqlitePool, visit_id: i64, count: usize) {
        use crate::db::repo::photo_records::{insert as insert_photo, PhotoRecordInput};
        let mut tx = pool.begin().await.unwrap();
        for i in 0..count {
            insert_photo(
                &mut tx,
                &PhotoRecordInput {
                    file_path: format!("C:/p/v{visit_id}-{i}.png").into(),
                    file_name: format!("v{visit_id}-{i}.png"),
                    taken_naive_local: nd(2026, 5, 9, 21, 0, 0),
                    taken_utc: chrono::TimeZone::from_utc_datetime(
                        &chrono::Utc,
                        &nd(2026, 5, 9, 12, 0, 0),
                    ),
                    taken_tz_id: "Asia/Tokyo".into(),
                    taken_offset_seconds: 32400,
                    taken_resolution: "Single".into(),
                    taken_tz_source: "CapturedRealtime".into(),
                    taken_resolution_confidence: "High".into(),
                    thumb_sha: None,
                    world_visit_id: Some(visit_id),
                },
            )
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn list_recent_with_photo_counts_returns_empty_for_clean_db() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();

        let rows = list_recent_with_photo_counts(&mut tx, 100).await.unwrap();

        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn list_recent_with_photo_counts_orders_by_joined_utc_descending() {
        // Arrange: 2 visits at different times → DESC で newest が先頭
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf = seed_pf(&mut tx).await;
        let raw1 = seed_raw(
            &mut tx,
            pf,
            0,
            LogEvent::RoomEntering {
                world_name: "A".into(),
            },
        )
        .await;
        let raw2 = seed_raw(
            &mut tx,
            pf,
            1,
            LogEvent::RoomEntering {
                world_name: "B".into(),
            },
        )
        .await;
        // visit A: joined 12:00 / left 12:30
        let v1 = insert_pending(&mut tx, raw1, "A", nd(2026, 5, 9, 12, 0, 0), ctx(), None)
            .await
            .unwrap();
        finalize_for_next_entering(&mut tx, v1, nd(2026, 5, 9, 12, 30, 0), ctx(), None)
            .await
            .unwrap();
        // visit B: joined 13:00 (newer)
        let _v2 = insert_pending(&mut tx, raw2, "B", nd(2026, 5, 9, 13, 0, 0), ctx(), None)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        let rows = list_recent_with_photo_counts(&mut tx, 100).await.unwrap();

        let names: Vec<_> = rows.iter().map(|v| v.world_name.as_str()).collect();
        assert_eq!(names, vec!["B", "A"], "joined_utc DESC で newest が先");
    }

    #[tokio::test]
    async fn list_recent_with_photo_counts_includes_zero_count_for_visits_without_photos() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf = seed_pf(&mut tx).await;
        let raw = seed_raw(
            &mut tx,
            pf,
            0,
            LogEvent::RoomEntering {
                world_name: "A".into(),
            },
        )
        .await;
        let _v = insert_pending(&mut tx, raw, "A", nd(2026, 5, 9, 12, 0, 0), ctx(), None)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        let rows = list_recent_with_photo_counts(&mut tx, 100).await.unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].photo_count, 0,
            "LEFT JOIN により photos 無しでも row 残る"
        );
    }

    #[tokio::test]
    async fn list_recent_with_photo_counts_counts_attached_photos_per_visit() {
        // Arrange: 2 visits、photo を 3 件と 0 件で attach
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf = seed_pf(&mut tx).await;
        let raw1 = seed_raw(
            &mut tx,
            pf,
            0,
            LogEvent::RoomEntering {
                world_name: "A".into(),
            },
        )
        .await;
        let raw2 = seed_raw(
            &mut tx,
            pf,
            1,
            LogEvent::RoomEntering {
                world_name: "B".into(),
            },
        )
        .await;
        let v1 = insert_pending(&mut tx, raw1, "A", nd(2026, 5, 9, 12, 0, 0), ctx(), None)
            .await
            .unwrap();
        let _v2 = insert_pending(&mut tx, raw2, "B", nd(2026, 5, 9, 13, 0, 0), ctx(), None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        attach_n_photos(&pool, v1, 3).await;
        // v2 には attach しない (photo_count=0)

        let mut tx = pool.begin().await.unwrap();
        let rows = list_recent_with_photo_counts(&mut tx, 100).await.unwrap();

        // newest first → B (count=0), A (count=3)
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].world_name, "B");
        assert_eq!(rows[0].photo_count, 0);
        assert_eq!(rows[1].world_name, "A");
        assert_eq!(rows[1].photo_count, 3);
    }

    #[tokio::test]
    async fn list_recent_with_photo_counts_returns_empty_when_limit_is_zero() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let pf = seed_pf(&mut tx).await;
        let raw = seed_raw(
            &mut tx,
            pf,
            0,
            LogEvent::RoomEntering {
                world_name: "A".into(),
            },
        )
        .await;
        let _ = insert_pending(&mut tx, raw, "A", nd(2026, 5, 9, 12, 0, 0), ctx(), None)
            .await
            .unwrap();

        let zero = list_recent_with_photo_counts(&mut tx, 0).await.unwrap();
        let neg = list_recent_with_photo_counts(&mut tx, -1).await.unwrap();

        assert!(zero.is_empty());
        assert!(neg.is_empty());
    }
}
