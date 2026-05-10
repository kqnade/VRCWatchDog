//! `photo_records` repository.
//!
//! VRChat の screenshot を 1 行 1 ファイルで持つテーブル。Phase 6 で実装する
//! PhotoScanner actor が EXIF/タイムスタンプ抽出後にこの repo を介して insert し、
//! photo_grid 画面が `list_recent` で表示する。
//!
//! 不変条件:
//! - `file_path` UNIQUE。同じファイルを scan しなおしても duplicate row は作らない。
//! - 親 `world_visits` への FK は `ON DELETE SET NULL` (visit が消えても photo は残る)。

use std::path::PathBuf;

use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::{Row, Sqlite, Transaction};

use crate::Result;

/// `insert` の入力。各 NOT NULL カラムにそのまま対応する。
///
/// `file_path` は OS path として渡し、内部で UTF-8 文字列に変換 (PathBuf::display ではなく
/// `to_string_lossy` を使うので、無効 UTF-8 バイトは置換文字に化けることに留意)。
#[derive(Debug, Clone)]
pub struct PhotoRecordInput {
    pub file_path: PathBuf,
    pub file_name: String,
    pub taken_naive_local: NaiveDateTime,
    pub taken_utc: DateTime<Utc>,
    pub taken_tz_id: String,
    pub taken_offset_seconds: i32,
    pub taken_resolution: String,
    pub taken_tz_source: String,
    pub taken_resolution_confidence: String,
    pub thumb_sha: Option<String>,
    pub world_visit_id: Option<i64>,
}

/// `list_recent` / `fetch_by_id` の戻り値。photo_grid に必要な最小フィールド。
///
/// EXIF tz の詳細 (offset/resolution/tz_source/confidence) は UI で要らないので省く。
/// 必要になったら別 read 関数を増やす。
///
/// `world_name` は `world_visits` を LEFT JOIN した非正規化フィールド (photo_grid /
/// /history のリンク UI で必須)。`world_visit_id` が NULL なら同じく NULL。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoRecord {
    pub id: i64,
    pub file_path: PathBuf,
    pub file_name: String,
    pub taken_naive_local: NaiveDateTime,
    pub taken_utc: DateTime<Utc>,
    pub thumb_sha: Option<String>,
    pub world_visit_id: Option<i64>,
    pub world_name: Option<String>,
}

/// 1 件 insert する。`file_path` UNIQUE で idempotent: 同 path の既存行があれば
/// 既存 id を返す (raw_log.rs と同じ pattern)。
///
/// 「同 path で metadata が違う場合に上書きする」挙動は意図的に入れていない。
/// 既存 metadata を尊重し、再 scan で書き換えたくなったら別関数 `upsert_overwrite` を
/// 後付けする想定。
pub async fn insert(tx: &mut Transaction<'_, Sqlite>, input: &PhotoRecordInput) -> Result<i64> {
    let file_path_str = input.file_path.to_string_lossy().into_owned();

    let row: (i64,) = sqlx::query_as(
        "INSERT INTO photo_records (
            file_path, file_name,
            taken_naive_local, taken_utc,
            taken_tz_id, taken_offset_seconds,
            taken_resolution, taken_tz_source, taken_resolution_confidence,
            thumb_sha, world_visit_id
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(file_path) DO UPDATE SET id = id
        RETURNING id",
    )
    .bind(&file_path_str)
    .bind(&input.file_name)
    .bind(
        input
            .taken_naive_local
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
    )
    .bind(input.taken_utc)
    .bind(&input.taken_tz_id)
    .bind(input.taken_offset_seconds)
    .bind(&input.taken_resolution)
    .bind(&input.taken_tz_source)
    .bind(&input.taken_resolution_confidence)
    .bind(input.thumb_sha.as_deref())
    .bind(input.world_visit_id)
    .fetch_one(&mut **tx)
    .await?;

    Ok(row.0)
}

/// 撮影日時 (`taken_utc`) の新しい順に最大 `limit` 件を返す。
///
/// `limit <= 0` の場合 SQLite は無制限扱いになるが、誤用防止のため呼び出し側が
/// 正の値を渡す前提。0 を渡しても空 vec が返るだけで panic はしない。
pub async fn list_recent(tx: &mut Transaction<'_, Sqlite>, limit: i64) -> Result<Vec<PhotoRecord>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }

    let rows = sqlx::query(
        "SELECT p.id, p.file_path, p.file_name,
                p.taken_naive_local, p.taken_utc,
                p.thumb_sha, p.world_visit_id,
                wv.world_name AS world_name
         FROM photo_records p
         LEFT JOIN world_visits wv ON wv.id = p.world_visit_id
         ORDER BY p.taken_utc DESC
         LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(row_to_photo_record(&row)?);
    }
    Ok(out)
}

/// `world_visit_id` に紐づく photo を `taken_utc` 降順で `limit` 件返す。
/// /history の visit 詳細パネルでサムネ表示するための read endpoint。
pub async fn list_for_visit(
    tx: &mut Transaction<'_, Sqlite>,
    visit_id: i64,
    limit: i64,
) -> Result<Vec<PhotoRecord>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT p.id, p.file_path, p.file_name,
                p.taken_naive_local, p.taken_utc,
                p.thumb_sha, p.world_visit_id,
                wv.world_name AS world_name
         FROM photo_records p
         LEFT JOIN world_visits wv ON wv.id = p.world_visit_id
         WHERE p.world_visit_id = ?1
         ORDER BY p.taken_utc DESC
         LIMIT ?2",
    )
    .bind(visit_id)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(row_to_photo_record(&row)?);
    }
    Ok(out)
}

/// row → `PhotoRecord` の共通変換 (list_recent / list_for_visit / fetch_by_id 共有)。
fn row_to_photo_record(row: &sqlx::sqlite::SqliteRow) -> Result<PhotoRecord> {
    let file_path_str: String = row.try_get("file_path")?;
    let taken_naive_local_str: String = row.try_get("taken_naive_local")?;
    let taken_utc: DateTime<Utc> = row.try_get("taken_utc")?;
    Ok(PhotoRecord {
        id: row.try_get("id")?,
        file_path: PathBuf::from(file_path_str),
        file_name: row.try_get("file_name")?,
        taken_naive_local: NaiveDateTime::parse_from_str(
            &taken_naive_local_str,
            "%Y-%m-%d %H:%M:%S",
        )
        .map_err(|e| crate::Error::Config(format!("invalid taken_naive_local: {e}")))?,
        taken_utc,
        thumb_sha: row.try_get("thumb_sha")?,
        world_visit_id: row.try_get("world_visit_id")?,
        world_name: row.try_get("world_name")?,
    })
}

/// `thumb_sha` がまだ埋まっていない (= サムネ未生成) row を id 昇順で最大 `limit` 件返す。
///
/// thumb_writer actor が worker loop で「次に処理すべき photo」を取り出すための query。
/// 戻り値は処理に必要な最小フィールド (id, file_path) のみで、photo_records 全列は読まない。
///
/// `limit <= 0` なら空 vec (`list_recent` と同じ防衛)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumblessPhotoRow {
    pub id: i64,
    pub file_path: PathBuf,
}

pub async fn list_thumbless(
    tx: &mut Transaction<'_, Sqlite>,
    limit: i64,
) -> Result<Vec<ThumblessPhotoRow>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }

    let rows = sqlx::query(
        "SELECT id, file_path
         FROM photo_records
         WHERE thumb_sha IS NULL
         ORDER BY id ASC
         LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let file_path_str: String = row.try_get("file_path")?;
        out.push(ThumblessPhotoRow {
            id: row.try_get("id")?,
            file_path: PathBuf::from(file_path_str),
        });
    }
    Ok(out)
}

/// `id` の photo_records 行の `thumb_sha` を更新する。
///
/// 該当 row が無い場合は no-op (rows_affected = 0)。
/// thumb_writer は処理完了後にこの関数で sha を書き戻す。
pub async fn update_thumb_sha(
    tx: &mut Transaction<'_, Sqlite>,
    id: i64,
    thumb_sha: &str,
) -> Result<u64> {
    let result = sqlx::query("UPDATE photo_records SET thumb_sha = ?1 WHERE id = ?2")
        .bind(thumb_sha)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected())
}

/// 単一 photo を id で引く。photo_grid のクリック → 詳細パネルや、open_photo command の
/// path 解決などで使う想定。
///
/// 見つからなければ `None`。
pub async fn fetch_by_id(tx: &mut Transaction<'_, Sqlite>, id: i64) -> Result<Option<PhotoRecord>> {
    let row = sqlx::query(
        "SELECT p.id, p.file_path, p.file_name,
                p.taken_naive_local, p.taken_utc,
                p.thumb_sha, p.world_visit_id,
                wv.world_name AS world_name
         FROM photo_records p
         LEFT JOIN world_visits wv ON wv.id = p.world_visit_id
         WHERE p.id = ?1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => Ok(Some(row_to_photo_record(&row)?)),
        None => Ok(None),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::open;
    use chrono::{NaiveDate, TimeZone};
    use std::path::Path;
    use tempfile::tempdir;

    /// テスト fixture: clean DB pool。tempdir は drop で削除されるので caller が保持する。
    async fn fresh_pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = open(&dir.path().join("test.db")).await.unwrap();
        (pool, dir)
    }

    /// `taken_*` 系を 1 引数で組み立てる input builder。
    /// path だけ呼び出し側で差し替えれば、複数 row テストが書きやすい。
    fn input_at(path: &Path, taken_utc: DateTime<Utc>) -> PhotoRecordInput {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed.png")
            .to_string();
        PhotoRecordInput {
            file_path: path.to_path_buf(),
            file_name,
            taken_naive_local: taken_utc.naive_utc(),
            taken_utc,
            taken_tz_id: "Asia/Tokyo".into(),
            taken_offset_seconds: 32400,
            taken_resolution: "Single".into(),
            taken_tz_source: "CapturedRealtime".into(),
            taken_resolution_confidence: "High".into(),
            thumb_sha: None,
            world_visit_id: None,
        }
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.from_utc_datetime(
            &NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, mi, s)
                .unwrap(),
        )
    }

    // -- insert -------------------------------------------------------------

    #[tokio::test]
    async fn insert_creates_row_and_returns_positive_id() {
        // Arrange
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let input = input_at(Path::new("C:/photos/a.png"), utc(2026, 5, 10, 12, 0, 0));

        // Act
        let id = insert(&mut tx, &input).await.unwrap();

        // Assert
        assert!(id > 0, "INTEGER PRIMARY KEY は autoincrement で 1 以上");
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn insert_duplicate_file_path_returns_existing_id_idempotently() {
        // Arrange: 同じ file_path を 2 回 insert する
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let input = input_at(Path::new("C:/photos/dup.png"), utc(2026, 5, 10, 12, 0, 0));

        // Act
        let first_id = insert(&mut tx, &input).await.unwrap();
        let second_id = insert(&mut tx, &input).await.unwrap();

        // Assert: 同 id を返し、行数は 1 件
        assert_eq!(
            first_id, second_id,
            "ON CONFLICT(file_path) DO UPDATE で既存 id が返る (idempotent)"
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM photo_records")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(count, 1, "duplicate insert で行は増えない");
    }

    // -- list_recent --------------------------------------------------------

    #[tokio::test]
    async fn list_recent_returns_empty_vec_for_clean_db() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();

        let rows = list_recent(&mut tx, 100).await.unwrap();

        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn list_recent_returns_rows_ordered_by_taken_utc_descending() {
        // Arrange: 3 件を意図的にバラバラの taken_utc 順で insert する
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let _ = insert(
            &mut tx,
            &input_at(Path::new("C:/p/middle.png"), utc(2026, 5, 10, 12, 0, 0)),
        )
        .await
        .unwrap();
        let _ = insert(
            &mut tx,
            &input_at(Path::new("C:/p/oldest.png"), utc(2026, 5, 9, 8, 0, 0)),
        )
        .await
        .unwrap();
        let _ = insert(
            &mut tx,
            &input_at(Path::new("C:/p/newest.png"), utc(2026, 5, 10, 18, 0, 0)),
        )
        .await
        .unwrap();

        // Act
        let rows = list_recent(&mut tx, 10).await.unwrap();

        // Assert: 新しい順 (newest, middle, oldest)
        let names: Vec<_> = rows.iter().map(|r| r.file_name.as_str()).collect();
        assert_eq!(names, vec!["newest.png", "middle.png", "oldest.png"]);
    }

    #[tokio::test]
    async fn list_recent_respects_limit_and_returns_only_n_newest_rows() {
        // Arrange: 5 件 insert、limit=2 なら新しい 2 件だけ
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        for i in 0..5 {
            let path = PathBuf::from(format!("C:/p/photo_{i}.png"));
            // i が大きいほど新しい
            let taken = utc(2026, 5, 10, 10 + i, 0, 0);
            insert(&mut tx, &input_at(&path, taken)).await.unwrap();
        }

        // Act
        let rows = list_recent(&mut tx, 2).await.unwrap();

        // Assert
        assert_eq!(rows.len(), 2);
        let names: Vec<_> = rows.iter().map(|r| r.file_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["photo_4.png", "photo_3.png"],
            "limit=2 なら最新 2 件だけ"
        );
    }

    #[tokio::test]
    async fn list_recent_returns_empty_when_limit_is_zero_or_negative() {
        // 防衛: limit が 0/負の場合に SQLite の `LIMIT 0` / `LIMIT -1` (無制限) という
        // 想定外動作を起こさないよう Rust 側で短絡している。
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        insert(
            &mut tx,
            &input_at(Path::new("C:/p/x.png"), utc(2026, 5, 10, 12, 0, 0)),
        )
        .await
        .unwrap();

        let zero = list_recent(&mut tx, 0).await.unwrap();
        let negative = list_recent(&mut tx, -1).await.unwrap();

        assert!(zero.is_empty());
        assert!(negative.is_empty());
    }

    // -- fetch_by_id --------------------------------------------------------

    #[tokio::test]
    async fn fetch_by_id_returns_none_for_unknown_id() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();

        let got = fetch_by_id(&mut tx, 9999).await.unwrap();

        assert!(got.is_none());
    }

    #[tokio::test]
    async fn fetch_by_id_returns_inserted_row_with_matching_fields() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let path = Path::new("C:/p/shot.png");
        let taken = utc(2026, 5, 10, 12, 0, 0);
        let id = insert(&mut tx, &input_at(path, taken)).await.unwrap();

        let got = fetch_by_id(&mut tx, id).await.unwrap().unwrap();

        assert_eq!(got.id, id);
        assert_eq!(got.file_path, path);
        assert_eq!(got.file_name, "shot.png");
        assert_eq!(got.taken_utc, taken);
        assert!(got.thumb_sha.is_none());
        assert!(got.world_visit_id.is_none());
    }

    // -- list_thumbless / update_thumb_sha (Phase 6.3.1) -----------------------

    #[tokio::test]
    async fn list_thumbless_returns_only_rows_with_null_thumb_sha() {
        // Arrange: 3 件 insert → 真ん中だけ thumb_sha を埋める
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let id_a = insert(
            &mut tx,
            &input_at(Path::new("C:/p/a.png"), utc(2026, 5, 10, 12, 0, 0)),
        )
        .await
        .unwrap();
        let _id_b = insert(
            &mut tx,
            &input_at(Path::new("C:/p/b.png"), utc(2026, 5, 10, 13, 0, 0)),
        )
        .await
        .unwrap();
        let id_c = insert(
            &mut tx,
            &input_at(Path::new("C:/p/c.png"), utc(2026, 5, 10, 14, 0, 0)),
        )
        .await
        .unwrap();
        update_thumb_sha(&mut tx, _id_b, "blake3-of-b")
            .await
            .unwrap();

        // Act
        let rows = list_thumbless(&mut tx, 100).await.unwrap();

        // Assert: a と c が残り、id 昇順
        let ids: Vec<_> = rows.iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec![id_a, id_c],
            "thumb_sha が NULL の行だけ id ASC で返る"
        );
    }

    #[tokio::test]
    async fn list_thumbless_respects_limit_and_returns_oldest_n_first() {
        // Arrange: 5 件 insert (全部 thumb_sha=NULL)
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        for i in 0..5 {
            insert(
                &mut tx,
                &input_at(
                    &PathBuf::from(format!("C:/p/{i}.png")),
                    utc(2026, 5, 10, 10 + i, 0, 0),
                ),
            )
            .await
            .unwrap();
        }

        // Act: limit=2 なら id 昇順で先頭 2 件
        let rows = list_thumbless(&mut tx, 2).await.unwrap();

        // Assert
        assert_eq!(rows.len(), 2);
        let names: Vec<_> = rows
            .iter()
            .map(|r| {
                r.file_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            names,
            vec!["0.png", "1.png"],
            "id 昇順で oldest 2 件 (= 古い photo を先に処理)"
        );
    }

    #[tokio::test]
    async fn list_thumbless_returns_empty_when_limit_is_zero_or_negative() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        insert(
            &mut tx,
            &input_at(Path::new("C:/p/x.png"), utc(2026, 5, 10, 12, 0, 0)),
        )
        .await
        .unwrap();

        let zero = list_thumbless(&mut tx, 0).await.unwrap();
        let neg = list_thumbless(&mut tx, -3).await.unwrap();

        assert!(zero.is_empty());
        assert!(neg.is_empty());
    }

    #[tokio::test]
    async fn update_thumb_sha_writes_value_and_returns_one_row_affected() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let id = insert(
            &mut tx,
            &input_at(Path::new("C:/p/x.png"), utc(2026, 5, 10, 12, 0, 0)),
        )
        .await
        .unwrap();

        let affected = update_thumb_sha(&mut tx, id, "deadbeef").await.unwrap();

        assert_eq!(affected, 1);
        let got = fetch_by_id(&mut tx, id).await.unwrap().unwrap();
        assert_eq!(got.thumb_sha.as_deref(), Some("deadbeef"));
    }

    // -- world_name JOIN / list_for_visit -------------------------------------

    /// raw_log_events / processed_log_files の FK を満たした上で world_visits に 1 行
    /// 入れて id を返す test helper (projector を経由しない直 INSERT)。
    /// `joined_utc` から決まる unique suffix で複数回呼べる。
    async fn seed_world_visit(
        pool: &sqlx::SqlitePool,
        world_name: &str,
        joined_utc: DateTime<Utc>,
    ) -> i64 {
        let unique_suffix = joined_utc.timestamp_millis();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO processed_log_files (
                file_identity_hash, log_sequence_key, volume_serial,
                file_id_high, file_id_low, generation, creation_time, first_kb_hash,
                file_name, file_size, mtime,
                ingest_position, last_projected_raw_event_id,
                tz_id, tz_source, processed_at
            ) VALUES (
                ?1, ?2, 0, 0, 0, 0, 0, 'k',
                'a.txt', 0, '2026-05-10T00:00:00Z',
                0, NULL, 'Asia/Tokyo', 'CapturedRealtime', '2026-05-10T00:00:00Z'
            )",
        )
        .bind(format!("h-{unique_suffix}"))
        .bind(format!("{unique_suffix}"))
        .execute(&mut *tx)
        .await
        .unwrap();
        let pf_id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM processed_log_files")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO raw_log_events (processed_log_file_id, byte_offset, event_type, payload_json)
             VALUES (?1, 0, 'RoomEntering', '{}')",
        )
        .bind(pf_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        let raw_id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM raw_log_events")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let visit_id: i64 = sqlx::query_scalar(
            "INSERT INTO world_visits (
                source_raw_event_id, world_name,
                resolution_state,
                joined_naive_local, joined_utc,
                joined_tz_id, joined_offset_seconds,
                joined_resolution, joined_tz_source, joined_resolution_confidence,
                left_utc
            ) VALUES (?1, ?2, 'Resolved', '2026-05-10 00:00:00', ?3,
                      'Asia/Tokyo', 32400, 'Single', 'CapturedRealtime', 'High', NULL)
            RETURNING id",
        )
        .bind(raw_id)
        .bind(world_name)
        .bind(joined_utc)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        visit_id
    }

    fn input_attached(path: &Path, taken_utc: DateTime<Utc>, visit_id: i64) -> PhotoRecordInput {
        let mut input = input_at(path, taken_utc);
        input.world_visit_id = Some(visit_id);
        input
    }

    #[tokio::test]
    async fn list_recent_returns_world_name_via_left_join_when_photo_attached_to_visit() {
        // Arrange: 1 visit + 1 attached photo + 1 unattached photo
        let (pool, _dir) = fresh_pool().await;
        let visit_id = seed_world_visit(&pool, "PartyRoom", utc(2026, 5, 10, 12, 0, 0)).await;
        let mut tx = pool.begin().await.unwrap();
        let _attached = insert(
            &mut tx,
            &input_attached(
                Path::new("C:/p/in.png"),
                utc(2026, 5, 10, 12, 30, 0),
                visit_id,
            ),
        )
        .await
        .unwrap();
        let _orphan = insert(
            &mut tx,
            &input_at(Path::new("C:/p/orphan.png"), utc(2026, 5, 10, 11, 0, 0)),
        )
        .await
        .unwrap();

        // Act
        let rows = list_recent(&mut tx, 10).await.unwrap();

        // Assert: 新しい (= attached) photo が world_name="PartyRoom"、orphan は None
        let by_name: std::collections::HashMap<&str, Option<&str>> = rows
            .iter()
            .map(|r| (r.file_name.as_str(), r.world_name.as_deref()))
            .collect();
        assert_eq!(by_name.get("in.png"), Some(&Some("PartyRoom")));
        assert_eq!(by_name.get("orphan.png"), Some(&None));
    }

    #[tokio::test]
    async fn list_for_visit_returns_only_photos_attached_to_given_visit_id() {
        // Arrange: 2 visits + 各々 attached photo + 1 orphan
        let (pool, _dir) = fresh_pool().await;
        let v1 = seed_world_visit(&pool, "WorldA", utc(2026, 5, 10, 10, 0, 0)).await;
        let v2 = seed_world_visit(&pool, "WorldB", utc(2026, 5, 10, 14, 0, 0)).await;
        let mut tx = pool.begin().await.unwrap();
        insert(
            &mut tx,
            &input_attached(Path::new("C:/p/a1.png"), utc(2026, 5, 10, 10, 30, 0), v1),
        )
        .await
        .unwrap();
        insert(
            &mut tx,
            &input_attached(Path::new("C:/p/a2.png"), utc(2026, 5, 10, 10, 45, 0), v1),
        )
        .await
        .unwrap();
        insert(
            &mut tx,
            &input_attached(Path::new("C:/p/b1.png"), utc(2026, 5, 10, 14, 30, 0), v2),
        )
        .await
        .unwrap();
        insert(
            &mut tx,
            &input_at(Path::new("C:/p/orphan.png"), utc(2026, 5, 10, 12, 0, 0)),
        )
        .await
        .unwrap();

        // Act
        let only_v1 = list_for_visit(&mut tx, v1, 10).await.unwrap();

        // Assert: WorldA の 2 件だけ、taken_utc 降順 (a2 → a1)
        let names: Vec<_> = only_v1.iter().map(|r| r.file_name.as_str()).collect();
        assert_eq!(names, vec!["a2.png", "a1.png"]);
    }

    #[tokio::test]
    async fn list_for_visit_returns_empty_when_no_photos_attached_to_visit() {
        let (pool, _dir) = fresh_pool().await;
        let visit_id = seed_world_visit(&pool, "EmptyWorld", utc(2026, 5, 10, 12, 0, 0)).await;
        let mut tx = pool.begin().await.unwrap();

        let rows = list_for_visit(&mut tx, visit_id, 10).await.unwrap();

        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn list_for_visit_returns_empty_when_limit_is_zero_or_negative() {
        let (pool, _dir) = fresh_pool().await;
        let visit_id = seed_world_visit(&pool, "W", utc(2026, 5, 10, 12, 0, 0)).await;
        let mut tx = pool.begin().await.unwrap();
        insert(
            &mut tx,
            &input_attached(
                Path::new("C:/p/x.png"),
                utc(2026, 5, 10, 12, 30, 0),
                visit_id,
            ),
        )
        .await
        .unwrap();

        let zero = list_for_visit(&mut tx, visit_id, 0).await.unwrap();
        let neg = list_for_visit(&mut tx, visit_id, -1).await.unwrap();

        assert!(zero.is_empty());
        assert!(neg.is_empty());
    }

    #[tokio::test]
    async fn update_thumb_sha_is_noop_for_unknown_id() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();

        let affected = update_thumb_sha(&mut tx, 9999, "deadbeef").await.unwrap();

        assert_eq!(affected, 0, "存在しない id は no-op (rows_affected = 0)");
    }
}
