//! `processed_log_files` テーブル用 repository。
//!
//! ログファイル単位の cursor (`ingest_position`, `last_projected_raw_event_id`)
//! とファイル識別子 (FileIdentity 5 フィールド + `log_sequence_key`) を保持。

use chrono::{DateTime, Utc};
use sqlx::{Sqlite, Transaction};

use crate::Result;

/// 新規ファイル登録のための入力。
///
/// `file_identity_hash` は呼び出し側で blake3 ハッシュ済み hex を渡す。
#[derive(Debug, Clone)]
pub struct ProcessedLogFileInput {
    pub file_identity_hash: String,
    pub log_sequence_key: String,
    pub volume_serial: u32,
    pub file_id_high: u32,
    pub file_id_low: u32,
    pub generation: u32,
    pub creation_time: i64,
    pub first_kb_hash: String,
    pub file_name: String,
    pub file_size: i64,
    pub mtime: DateTime<Utc>,
    pub tz_id: String,
    pub tz_source: String,
}

/// 新規 `processed_log_files` を upsert (file_identity_hash UNIQUE で衝突回避)。
///
/// 既存行があれば更新せず id だけを返す。`ingest_position` も触らない
/// (cursor は別 op で進めるため)。
pub async fn upsert(
    tx: &mut Transaction<'_, Sqlite>,
    input: &ProcessedLogFileInput,
) -> Result<i64> {
    let now = Utc::now();
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO processed_log_files (
            file_identity_hash, log_sequence_key,
            volume_serial, file_id_high, file_id_low, generation,
            creation_time, first_kb_hash,
            file_name, file_size, mtime,
            ingest_position, last_projected_raw_event_id,
            tz_id, tz_source, processed_at
        ) VALUES (
            ?1, ?2,
            ?3, ?4, ?5, ?6,
            ?7, ?8,
            ?9, ?10, ?11,
            0, NULL,
            ?12, ?13, ?14
        )
        ON CONFLICT(file_identity_hash) DO UPDATE SET
            file_size = excluded.file_size,
            mtime = excluded.mtime,
            file_name = excluded.file_name
        RETURNING id",
    )
    .bind(&input.file_identity_hash)
    .bind(&input.log_sequence_key)
    .bind(input.volume_serial as i64)
    .bind(input.file_id_high as i64)
    .bind(input.file_id_low as i64)
    .bind(input.generation as i64)
    .bind(input.creation_time)
    .bind(&input.first_kb_hash)
    .bind(&input.file_name)
    .bind(input.file_size)
    .bind(input.mtime.to_rfc3339())
    .bind(&input.tz_id)
    .bind(&input.tz_source)
    .bind(now.to_rfc3339())
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

/// 与えられた file の `ingest_position` を更新する。
pub async fn set_ingest_position(
    tx: &mut Transaction<'_, Sqlite>,
    file_id: i64,
    position: i64,
) -> Result<()> {
    sqlx::query("UPDATE processed_log_files SET ingest_position = ?1 WHERE id = ?2")
        .bind(position)
        .bind(file_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// 現在の `ingest_position` を取得する。テスト用。
pub async fn get_ingest_position(tx: &mut Transaction<'_, Sqlite>, file_id: i64) -> Result<i64> {
    let row: (i64,) =
        sqlx::query_as("SELECT ingest_position FROM processed_log_files WHERE id = ?1")
            .bind(file_id)
            .fetch_one(&mut **tx)
            .await?;
    Ok(row.0)
}
