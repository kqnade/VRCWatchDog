//! `player_sessions` repository。
//!
//! `OnPlayerJoined` で 1 行 insert、`OnPlayerLeft` で対応する未終了セッションの
//! `left_*` を埋める。マッチングキーは `(world_visit_id, user_id)` を優先、
//! `user_id` が無いログ (旧 VRChat) では `(world_visit_id, display_name)` で補完。

use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::{Row, Sqlite, Transaction};

use super::world_visits::TimeContext;
use crate::time::resolve_local_to_utc;
use crate::Result;

/// `list_for_visit` の戻り値。/history visit 詳細パネルで co-player 一覧表示用。
///
/// joined_utc / left_utc は UI で表示するだけなので tz 詳細 (offset / resolution) は省略。
/// `left_utc` が None なら「まだ visit から退室していない (= 同居中で終了)」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSessionView {
    pub id: i64,
    pub display_name: String,
    pub user_id: Option<String>,
    pub joined_utc: DateTime<Utc>,
    pub left_utc: Option<DateTime<Utc>>,
}

fn parse_tz(tz_id: &str) -> Result<chrono_tz::Tz> {
    tz_id
        .parse::<chrono_tz::Tz>()
        .map_err(|_| crate::Error::Config(format!("invalid tz: {tz_id}")))
}

fn fmt_naive(n: NaiveDateTime) -> String {
    n.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// `OnPlayerJoined` を受けて player_sessions に 1 行作成する。
/// `source_raw_event_id` UNIQUE で idempotent。
pub async fn insert_join(
    tx: &mut Transaction<'_, Sqlite>,
    raw_id: i64,
    world_visit_id: i64,
    display_name: &str,
    user_id: Option<&str>,
    naive_local: NaiveDateTime,
    ctx: TimeContext<'_>,
) -> Result<i64> {
    let existing: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM player_sessions WHERE source_raw_event_id = ?1")
            .bind(raw_id)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some((id,)) = existing {
        return Ok(id);
    }

    let tz = parse_tz(ctx.tz_id)?;
    let (utc, offset, res) = resolve_local_to_utc(naive_local, &tz, None);
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO player_sessions (
            source_raw_event_id, world_visit_id, display_name, user_id,
            joined_naive_local, joined_utc, joined_tz_id, joined_offset_seconds,
            joined_resolution, joined_tz_source, joined_resolution_confidence
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        RETURNING id",
    )
    .bind(raw_id)
    .bind(world_visit_id)
    .bind(display_name)
    .bind(user_id)
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

/// `OnPlayerLeft` で、対応する未終了 player_sessions の `left_*` を埋める。
/// マッチング: `(world_visit_id, user_id)` 一致が最優先、 user_id が NULL なら
/// `(world_visit_id, display_name)` で fallback。最新の未終了行 1 件のみ更新。
///
/// 戻り値は更新行数 (0 = 該当なし、 1 = 正常)。
pub async fn set_left(
    tx: &mut Transaction<'_, Sqlite>,
    world_visit_id: i64,
    display_name: &str,
    user_id: Option<&str>,
    naive_local: NaiveDateTime,
    ctx: TimeContext<'_>,
) -> Result<u64> {
    let tz = parse_tz(ctx.tz_id)?;
    let (utc, offset, res) = resolve_local_to_utc(naive_local, &tz, None);

    // user_id 一致を優先。 NULL の場合は display_name 一致で fallback。
    let target: Option<(i64,)> = if let Some(uid) = user_id {
        sqlx::query_as(
            "SELECT id FROM player_sessions
             WHERE world_visit_id = ?1 AND user_id = ?2 AND left_utc IS NULL
             ORDER BY joined_utc DESC LIMIT 1",
        )
        .bind(world_visit_id)
        .bind(uid)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id FROM player_sessions
             WHERE world_visit_id = ?1 AND display_name = ?2 AND left_utc IS NULL
             ORDER BY joined_utc DESC LIMIT 1",
        )
        .bind(world_visit_id)
        .bind(display_name)
        .fetch_optional(&mut **tx)
        .await?
    };
    let Some((session_id,)) = target else {
        return Ok(0);
    };

    let result = sqlx::query(
        "UPDATE player_sessions
         SET left_naive_local = ?1, left_utc = ?2,
             left_tz_id = ?3, left_offset_seconds = ?4,
             left_resolution = ?5, left_tz_source = ?6, left_resolution_confidence = ?7
         WHERE id = ?8",
    )
    .bind(fmt_naive(naive_local))
    .bind(utc.to_rfc3339())
    .bind(ctx.tz_id)
    .bind(offset)
    .bind(res.as_str())
    .bind(ctx.tz_source)
    .bind(ctx.resolution_confidence)
    .bind(session_id)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected())
}

/// 指定 visit の player_sessions を `joined_utc` 昇順 (= 入室順) で返す。
/// `limit <= 0` は空 vec (他 helpers と揃え)。
pub async fn list_for_visit(
    tx: &mut Transaction<'_, Sqlite>,
    visit_id: i64,
    limit: i64,
) -> Result<Vec<PlayerSessionView>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT id, display_name, user_id, joined_utc, left_utc
         FROM player_sessions
         WHERE world_visit_id = ?1
         ORDER BY joined_utc ASC
         LIMIT ?2",
    )
    .bind(visit_id)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(PlayerSessionView {
            id: row.try_get("id")?,
            display_name: row.try_get("display_name")?,
            user_id: row.try_get("user_id")?,
            joined_utc: row.try_get("joined_utc")?,
            left_utc: row.try_get("left_utc")?,
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
    use crate::db::repo::world_visits;
    use crate::log_parser::LogEvent;
    use chrono::{NaiveDate, Utc};
    use sqlx::Row;
    use tempfile::tempdir;

    async fn setup() -> (sqlx::SqlitePool, tempfile::TempDir) {
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

    fn nd(h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 5, 9)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    async fn seed_visit(pool: &sqlx::SqlitePool) -> i64 {
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
        let raw_id = raw_log::insert_batch_with_ledger(
            &mut tx,
            &[RawEventInput {
                processed_log_file_id: pf_id,
                byte_offset: 0,
                event: LogEvent::RoomEntering {
                    world_name: "Alpha".into(),
                },
                naive_local: Some(nd(20, 0, 0)),
            }],
        )
        .await
        .unwrap()[0];
        let visit_id =
            world_visits::insert_pending(&mut tx, raw_id, "Alpha", nd(20, 0, 0), ctx(), None)
                .await
                .unwrap();
        tx.commit().await.unwrap();
        visit_id
    }

    async fn seed_player_raw(pool: &sqlx::SqlitePool, offset: i64) -> i64 {
        let mut tx = pool.begin().await.unwrap();
        let pf_id: i64 = sqlx::query("SELECT id FROM processed_log_files LIMIT 1")
            .fetch_one(&mut *tx)
            .await
            .unwrap()
            .get(0);
        let raw_id = raw_log::insert_batch_with_ledger(
            &mut tx,
            &[RawEventInput {
                processed_log_file_id: pf_id,
                byte_offset: offset,
                event: LogEvent::PlayerJoined {
                    display_name: "Alice".into(),
                    user_id: Some("usr_aaa".into()),
                },
                naive_local: Some(nd(21, 0, 0)),
            }],
        )
        .await
        .unwrap()[0];
        tx.commit().await.unwrap();
        raw_id
    }

    /// 不変条件: PlayerJoined で session 行を作る、 source_raw_event_id で idempotent。
    #[tokio::test]
    async fn insert_join_is_idempotent_on_raw_id() {
        let (pool, _dir) = setup().await;
        let visit_id = seed_visit(&pool).await;
        let raw_id = seed_player_raw(&pool, 100).await;

        let mut tx = pool.begin().await.unwrap();
        let s1 = insert_join(
            &mut tx,
            raw_id,
            visit_id,
            "Alice",
            Some("usr_aaa"),
            nd(21, 0, 0),
            ctx(),
        )
        .await
        .unwrap();
        let s2 = insert_join(
            &mut tx,
            raw_id,
            visit_id,
            "Alice",
            Some("usr_aaa"),
            nd(21, 0, 0),
            ctx(),
        )
        .await
        .unwrap();
        assert_eq!(s1, s2);
        let count: i64 = sqlx::query("SELECT COUNT(*) FROM player_sessions")
            .fetch_one(&mut *tx)
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, 1);
    }

    /// 不変条件: PlayerLeft で同 user_id の未終了 session の left_* が埋まる。
    #[tokio::test]
    async fn set_left_finalizes_matching_session_by_user_id() {
        let (pool, _dir) = setup().await;
        let visit_id = seed_visit(&pool).await;
        let raw_id = seed_player_raw(&pool, 100).await;

        let mut tx = pool.begin().await.unwrap();
        insert_join(
            &mut tx,
            raw_id,
            visit_id,
            "Alice",
            Some("usr_aaa"),
            nd(21, 0, 0),
            ctx(),
        )
        .await
        .unwrap();
        let updated = set_left(
            &mut tx,
            visit_id,
            "Alice",
            Some("usr_aaa"),
            nd(22, 0, 0),
            ctx(),
        )
        .await
        .unwrap();
        assert_eq!(updated, 1);
        let row: (Option<String>,) =
            sqlx::query_as("SELECT left_utc FROM player_sessions WHERE world_visit_id = ?1")
                .bind(visit_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert!(row.0.is_some());
    }

    /// 不変条件: user_id 無し PlayerLeft は display_name fallback でマッチ。
    #[tokio::test]
    async fn set_left_falls_back_to_display_name_when_user_id_absent() {
        let (pool, _dir) = setup().await;
        let visit_id = seed_visit(&pool).await;
        let raw_id = seed_player_raw(&pool, 100).await;

        let mut tx = pool.begin().await.unwrap();
        // user_id 無しで insert
        insert_join(
            &mut tx,
            raw_id,
            visit_id,
            "Alice",
            None,
            nd(21, 0, 0),
            ctx(),
        )
        .await
        .unwrap();
        let updated = set_left(&mut tx, visit_id, "Alice", None, nd(22, 0, 0), ctx())
            .await
            .unwrap();
        assert_eq!(updated, 1);
    }

    // -- list_for_visit (Phase A4) -------------------------------------------

    /// 不変条件: list_for_visit は visit に紐づく session を joined_utc 昇順で返す。
    #[tokio::test]
    async fn list_for_visit_returns_co_players_in_join_order() {
        // Arrange: visit 1 つ + プレイヤー 3 人を joined 時刻バラバラに insert
        let (pool, _dir) = setup().await;
        let visit_id = seed_visit(&pool).await;
        let raw1 = seed_player_raw(&pool, 100).await;
        let raw2 = seed_player_raw(&pool, 200).await;
        let raw3 = seed_player_raw(&pool, 300).await;

        let mut tx = pool.begin().await.unwrap();
        // 後の時刻を先に insert して sort が効いていることを確認
        insert_join(
            &mut tx,
            raw1,
            visit_id,
            "Charlie",
            Some("usr_ccc"),
            nd(22, 0, 0),
            ctx(),
        )
        .await
        .unwrap();
        insert_join(
            &mut tx,
            raw2,
            visit_id,
            "Alice",
            Some("usr_aaa"),
            nd(20, 0, 0),
            ctx(),
        )
        .await
        .unwrap();
        insert_join(
            &mut tx,
            raw3,
            visit_id,
            "Bob",
            Some("usr_bbb"),
            nd(21, 0, 0),
            ctx(),
        )
        .await
        .unwrap();

        // Act
        let players = list_for_visit(&mut tx, visit_id, 100).await.unwrap();

        // Assert: joined_utc 昇順 = Alice (20), Bob (21), Charlie (22)
        let names: Vec<_> = players.iter().map(|p| p.display_name.as_str()).collect();
        assert_eq!(names, vec!["Alice", "Bob", "Charlie"]);
    }

    #[tokio::test]
    async fn list_for_visit_returns_empty_for_visit_with_no_players() {
        let (pool, _dir) = setup().await;
        let visit_id = seed_visit(&pool).await;
        let mut tx = pool.begin().await.unwrap();

        let players = list_for_visit(&mut tx, visit_id, 100).await.unwrap();

        assert!(players.is_empty());
    }

    #[tokio::test]
    async fn list_for_visit_returns_empty_when_limit_is_zero_or_negative() {
        let (pool, _dir) = setup().await;
        let visit_id = seed_visit(&pool).await;
        let raw = seed_player_raw(&pool, 100).await;
        let mut tx = pool.begin().await.unwrap();
        insert_join(
            &mut tx,
            raw,
            visit_id,
            "X",
            Some("usr_x"),
            nd(20, 0, 0),
            ctx(),
        )
        .await
        .unwrap();

        let zero = list_for_visit(&mut tx, visit_id, 0).await.unwrap();
        let neg = list_for_visit(&mut tx, visit_id, -1).await.unwrap();

        assert!(zero.is_empty());
        assert!(neg.is_empty());
    }

    /// 不変条件: PlayerLeft で該当する session が無いと 0 行更新。
    #[tokio::test]
    async fn set_left_returns_zero_when_no_match() {
        let (pool, _dir) = setup().await;
        let visit_id = seed_visit(&pool).await;
        let mut tx = pool.begin().await.unwrap();
        let updated = set_left(
            &mut tx,
            visit_id,
            "GhostUser",
            Some("usr_zzz"),
            nd(22, 0, 0),
            ctx(),
        )
        .await
        .unwrap();
        assert_eq!(updated, 0);
    }
}
