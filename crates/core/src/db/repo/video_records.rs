//! `video_records` repository。
//!
//! `[Video Playback] ... Attempting to resolve URL '<url>'` 行を 1 行 insert する。
//! title / thumbnail は Phase 7 で `video_info` サービスから補完する。

use chrono::NaiveDateTime;
use sqlx::{Sqlite, Transaction};

use super::world_visits::TimeContext;
use crate::time::resolve_local_to_utc;
use crate::Result;

fn parse_tz(tz_id: &str) -> Result<chrono_tz::Tz> {
    tz_id
        .parse::<chrono_tz::Tz>()
        .map_err(|_| crate::Error::Config(format!("invalid tz: {tz_id}")))
}

fn fmt_naive(n: NaiveDateTime) -> String {
    n.format("%Y-%m-%d %H:%M:%S").to_string()
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    tx: &mut Transaction<'_, Sqlite>,
    raw_id: i64,
    world_visit_id: Option<i64>,
    url: &str,
    naive_local: NaiveDateTime,
    ctx: TimeContext<'_>,
) -> Result<i64> {
    let existing: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM video_records WHERE source_raw_event_id = ?1")
            .bind(raw_id)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some((id,)) = existing {
        return Ok(id);
    }

    let tz = parse_tz(ctx.tz_id)?;
    let (utc, offset, res) = resolve_local_to_utc(naive_local, &tz, None);
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO video_records (
            source_raw_event_id, world_visit_id, url,
            detected_naive_local, detected_utc,
            detected_tz_id, detected_offset_seconds,
            detected_resolution, detected_tz_source, detected_resolution_confidence
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        RETURNING id",
    )
    .bind(raw_id)
    .bind(world_visit_id)
    .bind(url)
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
