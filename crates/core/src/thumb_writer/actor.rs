//! ThumbWriterActor: photo_records から `thumb_sha IS NULL` の行を batch 取り出し、
//! [`super::render::render_thumb_to_webp`] で webp 化し、
//! `<thumb_dir>/<sha>.webp` に書き出して `thumb_sha` を DB に書き戻す worker。
//!
//! plan §10 の "thumb_writer (各々別 actor)" に対応。
//!
//! 設計選択:
//! - **render は spawn_blocking**: image decode + resize + webp encode は CPU バウンド。
//!   tokio runtime を塞がないよう blocking pool に逃がす。
//! - **per-photo に独立 tx**: 1 件失敗が batch 全体を巻き戻すと、render 済みの bytes を
//!   再生成する hard cost が大きい。各 photo 独立に commit する方が hard fault に強い。
//! - **{sha}.webp 上書き許容**: blake3 は content-addressed なので、同 sha = 同 bytes。
//!   ファイルがすでにある場合の再書き込みは冪等 (= 同 bytes 上書き)、安心して再走できる。

use std::path::PathBuf;
use std::time::Duration;

use sqlx::SqlitePool;

use crate::db::repo::photo_records::{self, ThumblessPhotoRow};
use crate::thumb_writer::render::render_thumb_to_webp;
use crate::Result;

/// ThumbWriterActor の動作設定。
#[derive(Debug, Clone)]
pub struct ThumbWriterConfig {
    /// 出力ディレクトリ。`%LOCALAPPDATA%/VRCWatchDog/cache/thumbs` を想定。
    pub thumb_dir: PathBuf,
    /// 出力 webp の長辺最大画素数 (アスペクト比は保持)。
    pub max_dim: u32,
    /// 1 batch で取り出す件数。SQL 1 回 + render N 回の単位。
    pub batch_size: i64,
    /// batch が空だった時の poll 間隔。photo_scanner が新規行を入れるまでの待ち時間。
    pub poll_interval: Duration,
}

impl Default for ThumbWriterConfig {
    fn default() -> Self {
        Self {
            thumb_dir: PathBuf::new(), // 呼出側で必須上書き
            max_dim: 320,
            batch_size: 16,
            poll_interval: Duration::from_secs(5),
        }
    }
}

/// `process_batch` の戻り値。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchOutcome {
    /// list_thumbless で拾った件数。
    pub considered: usize,
    /// render + write + DB update まで成功した件数。
    pub succeeded: usize,
    /// 個別 photo で失敗した件数 (warn ログ済)。
    pub failed: usize,
}

pub struct ThumbWriterActor {
    pool: SqlitePool,
    config: ThumbWriterConfig,
}

impl ThumbWriterActor {
    pub fn new(pool: SqlitePool, config: ThumbWriterConfig) -> Self {
        Self { pool, config }
    }

    /// メインループ。空 batch なら poll_interval 待って retry。エラーは log だけして
    /// loop は止めない (1 件のおかしなファイルで全停止しない)。
    pub async fn run(self) -> Result<()> {
        loop {
            match self.process_batch().await {
                Ok(outcome) if outcome.considered > 0 => {
                    tracing::info!(
                        considered = outcome.considered,
                        succeeded = outcome.succeeded,
                        failed = outcome.failed,
                        "thumb_writer batch",
                    );
                    // 連続 batch がありえる (起動時 backlog) ので poll_interval 無し。
                    // ただし他 task に runtime を譲るため yield を挟む。
                    tokio::task::yield_now().await;
                }
                Ok(_) => {
                    // 空 batch。photo_scanner が新規行を入れるまで待つ。
                    tokio::time::sleep(self.config.poll_interval).await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "thumb_writer process_batch failed");
                    tokio::time::sleep(self.config.poll_interval).await;
                }
            }
        }
    }

    /// 1 batch を処理する。テスト・運用どちらでも 1 batch ずつ呼べる。
    pub async fn process_batch(&self) -> Result<BatchOutcome> {
        // Step 1: 取り出しは read-only tx で 1 回。
        let mut tx = self.pool.begin().await?;
        let rows = photo_records::list_thumbless(&mut tx, self.config.batch_size).await?;
        tx.commit().await?;

        if rows.is_empty() {
            return Ok(BatchOutcome {
                considered: 0,
                succeeded: 0,
                failed: 0,
            });
        }

        // Step 2: 各 photo を独立 tx で処理。1 件失敗が batch 全体を巻き戻さない。
        let mut succeeded = 0;
        let mut failed = 0;
        for row in &rows {
            match self.process_one(row).await {
                Ok(()) => succeeded += 1,
                Err(e) => {
                    tracing::warn!(
                        photo_id = row.id,
                        photo_path = %row.file_path.display(),
                        error = %e,
                        "thumb_writer process_one failed; skipping",
                    );
                    failed += 1;
                }
            }
        }

        Ok(BatchOutcome {
            considered: rows.len(),
            succeeded,
            failed,
        })
    }

    /// 1 photo を処理: render → write → DB update。
    /// pub にしてテストから直接呼べるようにしてある。
    pub async fn process_one(&self, row: &ThumblessPhotoRow) -> Result<()> {
        // Step 1: CPU バウンドな render を blocking pool で実行
        let file_path = row.file_path.clone();
        let max_dim = self.config.max_dim;
        let render_result =
            tokio::task::spawn_blocking(move || render_thumb_to_webp(&file_path, max_dim))
                .await
                .map_err(|e| crate::Error::Config(format!("spawn_blocking join: {e}")))?;
        let (bytes, sha) =
            render_result.map_err(|e| crate::Error::Config(format!("thumb render: {e}")))?;

        // Step 2: thumb_dir 確保 + ファイル書き込み (blake3 sha なので上書きは冪等)
        tokio::fs::create_dir_all(&self.config.thumb_dir).await?;
        let target = self.config.thumb_dir.join(format!("{sha}.webp"));
        tokio::fs::write(&target, &bytes).await?;

        // Step 3: DB に sha を書き戻し
        let mut tx = self.pool.begin().await?;
        photo_records::update_thumb_sha(&mut tx, row.id, &sha).await?;
        tx.commit().await?;

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::open;
    use crate::db::repo::photo_records::{insert, PhotoRecordInput};
    use chrono::{DateTime, NaiveDate, TimeZone, Utc};
    use image::{ImageBuffer, ImageFormat, Rgba};
    use std::path::Path;
    use tempfile::tempdir;

    /// テスト fixture: 空 DB pool + photo dir + thumb dir。
    async fn fresh_setup() -> (
        SqlitePool,
        tempfile::TempDir,
        tempfile::TempDir,
        tempfile::TempDir,
    ) {
        let db_dir = tempdir().unwrap();
        let pool = open(&db_dir.path().join("test.db")).await.unwrap();
        let photo_dir = tempdir().unwrap();
        let thumb_dir = tempdir().unwrap();
        (pool, db_dir, photo_dir, thumb_dir)
    }

    fn config_for(thumb_dir: &Path) -> ThumbWriterConfig {
        ThumbWriterConfig {
            thumb_dir: thumb_dir.to_path_buf(),
            max_dim: 64,
            batch_size: 16,
            poll_interval: Duration::from_millis(50),
        }
    }

    /// 指定サイズの test PNG を photo_dir 配下に書き出す。
    fn write_test_png(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_fn(w, h, |x, y| {
            Rgba([
                (x * 255 / w.max(1)) as u8,
                (y * 255 / h.max(1)) as u8,
                0,
                255,
            ])
        });
        let path = dir.join(name);
        img.save_with_format(&path, ImageFormat::Png).unwrap();
        path
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.from_utc_datetime(
            &NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, mi, s)
                .unwrap(),
        )
    }

    /// photo_records に 1 行 insert (画像ファイルは write_test_png で別途作る)。
    async fn seed_photo(pool: &SqlitePool, file_path: &Path) -> i64 {
        let mut tx = pool.begin().await.unwrap();
        let id = insert(
            &mut tx,
            &PhotoRecordInput {
                file_path: file_path.to_path_buf(),
                file_name: file_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                taken_naive_local: utc(2026, 5, 10, 12, 0, 0).naive_utc(),
                taken_utc: utc(2026, 5, 10, 12, 0, 0),
                taken_tz_id: "Asia/Tokyo".into(),
                taken_offset_seconds: 32400,
                taken_resolution: "Single".into(),
                taken_tz_source: "CapturedRealtime".into(),
                taken_resolution_confidence: "High".into(),
                thumb_sha: None,
                world_visit_id: None,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        id
    }

    async fn fetch_thumb_sha(pool: &SqlitePool, id: i64) -> Option<String> {
        sqlx::query_scalar("SELECT thumb_sha FROM photo_records WHERE id = ?1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    // -- process_batch ---------------------------------------------------------

    #[tokio::test]
    async fn process_batch_returns_zero_outcome_for_clean_db() {
        let (pool, _db, _photos, thumbs) = fresh_setup().await;
        let actor = ThumbWriterActor::new(pool, config_for(thumbs.path()));

        let outcome = actor.process_batch().await.unwrap();

        assert_eq!(
            outcome,
            BatchOutcome {
                considered: 0,
                succeeded: 0,
                failed: 0
            }
        );
    }

    #[tokio::test]
    async fn process_batch_writes_webp_file_and_updates_thumb_sha_for_each_photo() {
        // Arrange: 2 件 seed (両方有効な PNG)
        let (pool, _db, photos, thumbs) = fresh_setup().await;
        let p1 = write_test_png(photos.path(), "a.png", 100, 100);
        let p2 = write_test_png(photos.path(), "b.png", 200, 100);
        let id1 = seed_photo(&pool, &p1).await;
        let id2 = seed_photo(&pool, &p2).await;
        let actor = ThumbWriterActor::new(pool.clone(), config_for(thumbs.path()));

        // Act
        let outcome = actor.process_batch().await.unwrap();

        // Assert: 2 件成功
        assert_eq!(outcome.considered, 2);
        assert_eq!(outcome.succeeded, 2);
        assert_eq!(outcome.failed, 0);

        // DB に sha が入った
        let sha1 = fetch_thumb_sha(&pool, id1).await.unwrap();
        let sha2 = fetch_thumb_sha(&pool, id2).await.unwrap();
        assert_eq!(sha1.len(), 64);
        assert_eq!(sha2.len(), 64);
        assert_ne!(sha1, sha2, "異なる画像なら異なる sha");

        // ファイルも書かれた
        assert!(thumbs.path().join(format!("{sha1}.webp")).is_file());
        assert!(thumbs.path().join(format!("{sha2}.webp")).is_file());
    }

    #[tokio::test]
    async fn process_batch_skips_already_thumbed_rows() {
        // Arrange: seed → 1 回目で sha 書き込み済 → 2 回目は何もしない
        let (pool, _db, photos, thumbs) = fresh_setup().await;
        let p = write_test_png(photos.path(), "a.png", 100, 100);
        let _id = seed_photo(&pool, &p).await;
        let actor = ThumbWriterActor::new(pool.clone(), config_for(thumbs.path()));
        let _ = actor.process_batch().await.unwrap();

        // Act: 2 回目
        let second = actor.process_batch().await.unwrap();

        // Assert
        assert_eq!(
            second,
            BatchOutcome {
                considered: 0,
                succeeded: 0,
                failed: 0
            },
            "1 回目で thumb_sha が埋まったので 2 回目は空 batch",
        );
    }

    #[tokio::test]
    async fn process_batch_continues_after_a_single_photo_render_error() {
        // Arrange: 2 件 seed、1 件は実 PNG、もう 1 件は存在しない path
        let (pool, _db, photos, thumbs) = fresh_setup().await;
        let p_ok = write_test_png(photos.path(), "ok.png", 100, 100);
        let p_missing = photos.path().join("does_not_exist.png");
        let id_ok = seed_photo(&pool, &p_ok).await;
        let id_missing = seed_photo(&pool, &p_missing).await;
        let actor = ThumbWriterActor::new(pool.clone(), config_for(thumbs.path()));

        // Act
        let outcome = actor.process_batch().await.unwrap();

        // Assert
        assert_eq!(outcome.considered, 2);
        assert_eq!(outcome.succeeded, 1);
        assert_eq!(outcome.failed, 1);

        // 成功した方は sha 入り、失敗した方は NULL のまま
        assert!(fetch_thumb_sha(&pool, id_ok).await.is_some());
        assert!(fetch_thumb_sha(&pool, id_missing).await.is_none());
    }

    #[tokio::test]
    async fn process_batch_respects_batch_size_limit() {
        // Arrange: 5 件 seed、batch_size=2 → 1 回の process_batch では 2 件だけ処理
        let (pool, _db, photos, thumbs) = fresh_setup().await;
        for i in 0..5 {
            let p = write_test_png(photos.path(), &format!("{i}.png"), 100, 100);
            seed_photo(&pool, &p).await;
        }
        let mut cfg = config_for(thumbs.path());
        cfg.batch_size = 2;
        let actor = ThumbWriterActor::new(pool, cfg);

        let outcome = actor.process_batch().await.unwrap();

        assert_eq!(outcome.considered, 2);
        assert_eq!(outcome.succeeded, 2);
    }

    // -- process_one (内部 entry の単独テスト) -------------------------------

    #[tokio::test]
    async fn process_one_writes_thumb_file_named_after_blake3_sha() {
        let (pool, _db, photos, thumbs) = fresh_setup().await;
        let p = write_test_png(photos.path(), "x.png", 50, 50);
        let id = seed_photo(&pool, &p).await;
        let actor = ThumbWriterActor::new(pool.clone(), config_for(thumbs.path()));

        actor
            .process_one(&ThumblessPhotoRow {
                id,
                file_path: p.clone(),
            })
            .await
            .unwrap();

        let sha = fetch_thumb_sha(&pool, id).await.unwrap();
        let written_path = thumbs.path().join(format!("{sha}.webp"));
        assert!(
            written_path.is_file(),
            "thumb file は <thumb_dir>/<sha>.webp に書かれる: {}",
            written_path.display()
        );
        // ファイル先頭は webp signature (RIFF....WEBP)
        let bytes = std::fs::read(&written_path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
    }
}
