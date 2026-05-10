//! 写真 1 件 (= path) を [`crate::db::repo::photo_records`] に投入する pure-async ロジック。
//!
//! 中身を actor から切り離してあるので、file-backed SQLite を持ち込むだけで完結に
//! テストできる。`PhotoScannerActor` (Phase 6.1.3b) は `notify` から得た path を
//! batch ごとにこの関数に流すだけ。

use std::path::Path;

use chrono::DateTime;
use chrono_tz::Tz;
use sqlx::{Sqlite, Transaction};

use crate::db::repo::photo_records::{self, PhotoRecordInput};
use crate::photo_scanner::filename::parse_vrchat_filename;
use crate::photo_scanner::visit_matcher::{match_photo_to_visit, WorldVisitTimeRange};
use crate::time::resolve_local_to_utc;
use crate::Result;

/// `ingest_photo` の戻り値。actor 側で tracing/metrics に落とすときの分類になる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    /// photo_records に row が存在することが保証されている (新規 insert か既存ヒットかは
    /// 区別しない)。`world_visit_id` はこの insert で attach した visit (None なら未マッチ)。
    Recorded {
        id: i64,
        world_visit_id: Option<i64>,
    },
    /// 取り込み対象外と判定して何もしなかった。理由 enum 付き。
    Skipped(SkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// `path.file_name()` が `None` (root path 等)。
    NoFileNameComponent,
    /// ファイル名が non-UTF-8。Windows のレガシー Shift-JIS 名は対象外。
    InvalidUtf8FileName,
    /// VRChat 命名規則 (`VRChat_YYYY-MM-DD_HH-MM-SS...`) にマッチしない。
    NotVRChatFilename,
    /// 拡張子 `.tmp` (VRChat の write-in-flight)。
    InflightTmp,
}

/// 1 件の photo path を取り込む。outcome を返し、エラーは DB / 時刻 resolve 由来のみ。
///
/// 引数:
/// - `tx`: caller が保持する書き込み tx。複数 photo を 1 batch にまとめる想定。
/// - `file_path`: 対象 photo の絶対 path (notify が渡してくる形を想定)。
/// - `visits`: `joined_utc` ASC で sort 済みの WorldVisitTimeRange。actor が batch 開始
///   時に DB から 1 度 load してこの関数に何度も渡す。
/// - `tz`: photo の `taken_naive_local` を UTC に解決する前提のタイムゾーン。
///   `iana_time_zone::get_timezone()` で OS から取得した tz (= 撮影機の tz と仮定)。
///
/// 動作:
/// 1. file_name を抽出。失敗なら Skipped。
/// 2. `.tmp` を skip。
/// 3. VRChat 命名規則を parser でチェック。non-VRChat なら Skipped (= Discord screenshot
///    等の混入を排除)。
/// 4. taken_naive_local を `tz` で UTC に解決。
/// 5. `match_photo_to_visit` で world_visit_id を引く。
/// 6. `photo_records::insert` を呼ぶ (file_path UNIQUE で idempotent)。
pub async fn ingest_photo(
    tx: &mut Transaction<'_, Sqlite>,
    file_path: &Path,
    visits: &[WorldVisitTimeRange],
    tz: &Tz,
) -> Result<IngestOutcome> {
    // Step 1: file_name を取り出す
    let file_name_os = match file_path.file_name() {
        Some(n) => n,
        None => return Ok(IngestOutcome::Skipped(SkipReason::NoFileNameComponent)),
    };
    let file_name = match file_name_os.to_str() {
        Some(s) => s,
        None => return Ok(IngestOutcome::Skipped(SkipReason::InvalidUtf8FileName)),
    };

    // Step 2: .tmp は明示的に skip
    if file_name.to_ascii_lowercase().ends_with(".tmp") {
        return Ok(IngestOutcome::Skipped(SkipReason::InflightTmp));
    }

    // Step 3: VRChat 命名規則チェック
    let Some(meta) = parse_vrchat_filename(file_name) else {
        return Ok(IngestOutcome::Skipped(SkipReason::NotVRChatFilename));
    };

    // Step 4: tz 解決
    // photo は単発で連続性が無いので prev_utc は None (DST 境界での monotonic 補正は不要)。
    let (taken_utc, offset_seconds, resolution) =
        resolve_local_to_utc(meta.taken_naive_local, tz, None);

    // Step 5: visit マッチ
    let world_visit_id = match_photo_to_visit(visits, taken_utc);

    // Step 6: insert (idempotent on file_path)
    let id = photo_records::insert(
        tx,
        &PhotoRecordInput {
            file_path: file_path.to_path_buf(),
            file_name: file_name.to_string(),
            taken_naive_local: meta.taken_naive_local,
            taken_utc,
            taken_tz_id: tz.name().to_string(),
            taken_offset_seconds: offset_seconds,
            taken_resolution: resolution.as_str().to_string(),
            // 撮影タイミングに OS から取った tz を使うので tz_source は CapturedRealtime。
            taken_tz_source: "CapturedRealtime".to_string(),
            // photo は EXIF 解析等の二段階推論をしていないので Confidence は High 相当。
            taken_resolution_confidence: "High".to_string(),
            // thumb は thumb_writer (Phase 6.3) が後で埋める
            thumb_sha: None,
            world_visit_id,
        },
    )
    .await?;

    Ok(IngestOutcome::Recorded { id, world_visit_id })
}

/// `taken_utc` の前後 1 visit を即取れるように `world_visits` を `joined_utc` ASC で
/// load するヘルパー。actor は batch ごとにこれを 1 回呼んで、結果を `ingest_photo` に
/// 何度も渡す想定。
///
/// `left_utc` は NULL (= ongoing visit) を `None` にマップ。
pub async fn load_world_visit_ranges(
    tx: &mut Transaction<'_, Sqlite>,
) -> Result<Vec<WorldVisitTimeRange>> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT id, joined_utc, left_utc
         FROM world_visits
         ORDER BY joined_utc ASC",
    )
    .fetch_all(&mut **tx)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(WorldVisitTimeRange {
            id: row.try_get("id")?,
            joined_utc: row.try_get::<DateTime<chrono::Utc>, _>("joined_utc")?,
            left_utc: row.try_get::<Option<DateTime<chrono::Utc>>, _>("left_utc")?,
        });
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::open;
    use chrono::{NaiveDate, TimeZone, Utc};
    use std::path::PathBuf;
    use tempfile::tempdir;

    async fn fresh_pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = open(&dir.path().join("test.db")).await.unwrap();
        (pool, dir)
    }

    fn jst() -> Tz {
        "Asia/Tokyo".parse().unwrap()
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.from_utc_datetime(
            &NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, mi, s)
                .unwrap(),
        )
    }

    /// world_visits に visit を 1 件 seed して id を返す helper (raw event 経由で)。
    /// 同 pool に複数回呼べるよう、processed_log_files の UNIQUE 列 (file_identity_hash /
    /// log_sequence_key) を joined_utc 由来でユニーク化している。
    async fn seed_world_visit(
        pool: &sqlx::SqlitePool,
        joined_utc: DateTime<Utc>,
        left_utc: Option<DateTime<Utc>>,
    ) -> i64 {
        // 直接 INSERT (test 用のショートカット — 実 pipeline は projector が world_visits を作る)。
        // FK 都合で processed_log_files + raw_log_events も入れる。
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
            ) VALUES (?1, 'TestWorld', 'Resolved', '2026-05-10 00:00:00', ?2,
                      'Asia/Tokyo', 32400, 'Single', 'CapturedRealtime', 'High', ?3)
            RETURNING id",
        )
        .bind(raw_id)
        .bind(joined_utc)
        .bind(left_utc)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        visit_id
    }

    // -- happy path -------------------------------------------------------------

    #[tokio::test]
    async fn ingest_records_vrchat_photo_with_no_visits_to_attach() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let path = PathBuf::from("C:/photos/VRChat_2026-05-10_12-34-56.789_1920x1080.png");

        let outcome = ingest_photo(&mut tx, &path, &[], &jst()).await.unwrap();

        match outcome {
            IngestOutcome::Recorded { id, world_visit_id } => {
                assert!(id > 0);
                assert!(world_visit_id.is_none(), "visits 空なら attach されない");
            }
            other => panic!("expected Recorded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ingest_attaches_world_visit_id_when_photo_taken_during_visit() {
        let (pool, _dir) = fresh_pool().await;
        // visit: 2026-05-10 12:00:00 〜 13:00:00 (UTC で記録)
        let visit_id = seed_world_visit(
            &pool,
            utc(2026, 5, 10, 12, 0, 0),
            Some(utc(2026, 5, 10, 13, 0, 0)),
        )
        .await;

        let mut tx = pool.begin().await.unwrap();
        let visits = load_world_visit_ranges(&mut tx).await.unwrap();
        // 写真の filename は JST 21-30 = UTC 12:30 → visit 内
        let path = PathBuf::from("C:/photos/VRChat_2026-05-10_21-30-00_1920x1080.png");

        let outcome = ingest_photo(&mut tx, &path, &visits, &jst()).await.unwrap();

        let IngestOutcome::Recorded { world_visit_id, .. } = outcome else {
            panic!("expected Recorded");
        };
        assert_eq!(world_visit_id, Some(visit_id));
    }

    #[tokio::test]
    async fn ingest_is_idempotent_on_same_file_path() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let path = PathBuf::from("C:/photos/VRChat_2026-05-10_12-34-56_1920x1080.png");

        let first = ingest_photo(&mut tx, &path, &[], &jst()).await.unwrap();
        let second = ingest_photo(&mut tx, &path, &[], &jst()).await.unwrap();

        let IngestOutcome::Recorded { id: id1, .. } = first else {
            panic!("first should be Recorded");
        };
        let IngestOutcome::Recorded { id: id2, .. } = second else {
            panic!("second should be Recorded");
        };
        assert_eq!(id1, id2, "file_path UNIQUE で同一 id を返す");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM photo_records")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    // -- skip cases -------------------------------------------------------------

    #[tokio::test]
    async fn ingest_skips_non_vrchat_filename() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let path = PathBuf::from("C:/photos/Discord_2026-05-10_12-34-56.png");

        let outcome = ingest_photo(&mut tx, &path, &[], &jst()).await.unwrap();

        assert_eq!(
            outcome,
            IngestOutcome::Skipped(SkipReason::NotVRChatFilename)
        );
    }

    #[tokio::test]
    async fn ingest_skips_tmp_extension_inflight_file() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        // VRChat 命名規則だが拡張子が .tmp (write 中の状態)
        let path = PathBuf::from("C:/photos/VRChat_2026-05-10_12-34-56.tmp");

        let outcome = ingest_photo(&mut tx, &path, &[], &jst()).await.unwrap();

        assert_eq!(outcome, IngestOutcome::Skipped(SkipReason::InflightTmp));
    }

    #[tokio::test]
    async fn ingest_skips_path_with_no_file_name_component() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();
        // root path には file_name が無い (Windows: "C:\\", Unix: "/")
        let root = PathBuf::from("/");

        let outcome = ingest_photo(&mut tx, &root, &[], &jst()).await.unwrap();

        // Windows ローカルでは root は "/" を file_name 扱いにしないので NoFileNameComponent。
        // (各 OS の挙動で `path.file_name()` が `None` を返すケース全般)
        assert_eq!(
            outcome,
            IngestOutcome::Skipped(SkipReason::NoFileNameComponent)
        );
    }

    // -- DB 副作用なし: skip 時 -------------------------------------------------

    #[tokio::test]
    async fn ingest_does_not_insert_photo_record_when_skipped() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();

        for path in [
            "C:/photos/Discord_2026-05-10_12-34-56.png",
            "C:/photos/VRChat_2026-05-10_12-34-56.tmp",
        ] {
            let _ = ingest_photo(&mut tx, &PathBuf::from(path), &[], &jst())
                .await
                .unwrap();
        }

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM photo_records")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(count, 0, "skip 時は photo_records に行が増えない");
    }

    // -- load_world_visit_ranges helper ---------------------------------------

    #[tokio::test]
    async fn load_world_visit_ranges_returns_empty_for_clean_db() {
        let (pool, _dir) = fresh_pool().await;
        let mut tx = pool.begin().await.unwrap();

        let visits = load_world_visit_ranges(&mut tx).await.unwrap();

        assert!(visits.is_empty());
    }

    #[tokio::test]
    async fn load_world_visit_ranges_returns_visits_sorted_by_joined_utc_ascending() {
        let (pool, _dir) = fresh_pool().await;
        // 後の時刻を先に seed して、sort が効いていることを確認する
        let later = seed_world_visit(
            &pool,
            utc(2026, 5, 10, 14, 0, 0),
            Some(utc(2026, 5, 10, 15, 0, 0)),
        )
        .await;
        let earlier = seed_world_visit(
            &pool,
            utc(2026, 5, 10, 12, 0, 0),
            Some(utc(2026, 5, 10, 13, 0, 0)),
        )
        .await;

        let mut tx = pool.begin().await.unwrap();
        let visits = load_world_visit_ranges(&mut tx).await.unwrap();

        let ids: Vec<_> = visits.iter().map(|v| v.id).collect();
        assert_eq!(
            ids,
            vec![earlier, later],
            "joined_utc ASC で並ぶ (insert 順ではない)"
        );
    }

    #[tokio::test]
    async fn load_world_visit_ranges_preserves_null_left_utc_as_none() {
        let (pool, _dir) = fresh_pool().await;
        // ongoing visit (left_utc NULL)
        let _id = seed_world_visit(&pool, utc(2026, 5, 10, 12, 0, 0), None).await;

        let mut tx = pool.begin().await.unwrap();
        let visits = load_world_visit_ranges(&mut tx).await.unwrap();

        assert_eq!(visits.len(), 1);
        assert!(
            visits[0].left_utc.is_none(),
            "ongoing visit は Option::None"
        );
    }
}
