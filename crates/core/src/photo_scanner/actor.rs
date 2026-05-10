//! `PhotoScannerActor`: notify event を受けて [`crate::photo_scanner::ingest::ingest_photo`]
//! に流す actor。
//!
//! [`crate::log_watcher::FsEvent`] / [`crate::log_watcher::FsEventSource`] を再利用。
//! 「path 変化を 1 件受けて DB に流す」という最小単位は同じなので、abstract を共有して
//! テスト fake 等の重複を避ける。
//!
//! 設計選択:
//! - **1 event = 1 transaction**: notify-debouncer-mini が既に 500ms debounce している
//!   ので、tx 開く頻度はそもそも低い。複数 event を 1 tx に束ねる最適化は実 trace で
//!   ボトルネックが見えてからで十分。
//! - **reconcile は 1 transaction で全件**: 起動時の catch-up や overflow 後の再スキャン
//!   では数百〜数千ファイルになりうるため、1 ファイル 1 tx だと commit fsync が
//!   ボトルネック。世界 visits の load も 1 度で済ませる。
//! - **PathRemoved は何もしない**: photo_records は履歴目的なので、ファイルが消えても
//!   row は残す。UI で 404 になったら別途「再スキャン」操作を提供する想定。

use std::path::{Path, PathBuf};

use chrono_tz::Tz;
use sqlx::SqlitePool;

use crate::log_watcher::{FsEvent, FsEventSource};
use crate::photo_scanner::ingest::{ingest_photo, load_world_visit_ranges, IngestOutcome};
use crate::Result;

/// PhotoScannerActor の動作設定。
#[derive(Debug, Clone)]
pub struct PhotoScannerConfig {
    /// 写真の `taken_naive_local` を UTC に解決する前提のタイムゾーン。
    /// 通常は OS の local tz を `iana_time_zone::get_timezone()` から起動時に取って渡す。
    pub tz: Tz,
}

/// `handle_event` の戻り値。actor 内部の tracing/metrics 用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandleOutcome {
    /// PathChanged を 1 件 ingest した。中身は [`IngestOutcome`] そのまま。
    Ingest(IngestOutcome),
    /// PathRemoved を受け取った (本 actor では noop)。
    Removed,
    /// QueueOverflow を ack した (reconcile は呼出し側で行う)。
    OverflowAck,
}

/// `reconcile` の戻り値。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileOutcome {
    /// scan 対象になった画像ファイル数 (拡張子フィルタ通過後)。
    pub considered: usize,
    /// うち photo_records に新規 / 既存 row として記録されたもの。
    pub recorded: usize,
    /// VRChat 命名規則に合わない / .tmp 等で skip された数。
    pub skipped: usize,
}

/// PhotoScannerActor。`run()` で source が drain するまで loop する。
pub struct PhotoScannerActor<S: FsEventSource> {
    pool: SqlitePool,
    source: S,
    config: PhotoScannerConfig,
}

impl<S: FsEventSource> PhotoScannerActor<S> {
    pub fn new(pool: SqlitePool, source: S, config: PhotoScannerConfig) -> Self {
        Self {
            pool,
            source,
            config,
        }
    }

    /// 1 イベントを処理する。テスト・実運用どちらでも 1 イベントずつ呼べる。
    pub async fn handle_event(&mut self, event: FsEvent) -> Result<HandleOutcome> {
        match event {
            FsEvent::PathChanged(path) => self.ingest_path(&path).await,
            FsEvent::PathRemoved(_) => Ok(HandleOutcome::Removed),
            FsEvent::QueueOverflow => Ok(HandleOutcome::OverflowAck),
        }
    }

    /// メインループ。`source` が `None` を返すまで回す。
    /// 個別 event の失敗は `tracing::warn` してループ継続 (1 件の壊れたファイルで全停止しない)。
    pub async fn run(mut self) -> Result<()> {
        while let Some(event) = self.source.next().await {
            match self.handle_event(event).await {
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "photo_scanner event failed"),
            }
        }
        Ok(())
    }

    /// dir を **再帰的に** list して、画像拡張子のものすべてを 1 transaction で ingest する。
    ///
    /// 起動時 catch-up + notify overflow 後の rescan で使う。
    /// VRChat デフォルトの保存先は `Pictures/VRChat/YYYY-MM/VRChat_*.png` のように
    /// 月別サブディレクトリで構成されているため、ユーザーが指定する photo_directory は
    /// 親ディレクトリで、写真本体は 1 段下にある。再帰スキャンが必須。
    /// 数百〜数千件になりうるが、1 tx + visits 1 回 load なので overhead は最小。
    pub async fn reconcile(&mut self, dir: &Path) -> Result<ReconcileOutcome> {
        // Step 1: root の存在チェック (NotFound は設定ミス / 初回未起動 VRChat。silent skip)。
        match tokio::fs::metadata(dir).await {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Ok(ReconcileOutcome {
                    considered: 0,
                    recorded: 0,
                    skipped: 0,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ReconcileOutcome {
                    considered: 0,
                    recorded: 0,
                    skipped: 0,
                });
            }
            Err(e) => return Err(e.into()),
        }

        // Step 2: 再帰スキャンで画像のみ集める
        let paths = collect_image_files_recursive(dir).await?;
        if paths.is_empty() {
            return Ok(ReconcileOutcome {
                considered: 0,
                recorded: 0,
                skipped: 0,
            });
        }

        // Step 3: 1 tx で visits 1 回 load + 全件 ingest
        let mut tx = self.pool.begin().await?;
        let visits = load_world_visit_ranges(&mut tx).await?;
        let mut recorded = 0;
        let mut skipped = 0;
        for path in &paths {
            match ingest_photo(&mut tx, path, &visits, &self.config.tz).await? {
                IngestOutcome::Recorded { .. } => recorded += 1,
                IngestOutcome::Skipped(_) => skipped += 1,
            }
        }
        tx.commit().await?;

        Ok(ReconcileOutcome {
            considered: paths.len(),
            recorded,
            skipped,
        })
    }

    /// 1 path を別 tx で ingest する内部ヘルパ (handle_event::PathChanged 用)。
    async fn ingest_path(&mut self, path: &Path) -> Result<HandleOutcome> {
        let mut tx = self.pool.begin().await?;
        let visits = load_world_visit_ranges(&mut tx).await?;
        let outcome = ingest_photo(&mut tx, path, &visits, &self.config.tz).await?;
        tx.commit().await?;
        Ok(HandleOutcome::Ingest(outcome))
    }
}

/// `root` 以下の image ファイルを再帰収集する (BFS)。
///
/// VRChat デフォルトの `Pictures/VRChat/YYYY-MM/*.png` 構造に対応するため、
/// reconcile からは root + サブディレクトリ全部を 1 回で list する必要がある。
///
/// 動作:
/// - `tokio::fs::read_dir` をスタックで回す (再帰呼び出しは避けて深さ無制限で安全)。
/// - 隠しディレクトリ (名前が `.` で始まる) はスキップ — `.thumbnails`, `.trash`,
///   `.git` 等のメタデータが混入しない。
/// - サブ dir 単位の NotFound (途中で消えた) は continue で吸収、loop 全体は止めない。
async fn collect_image_files_recursive(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            let ft = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() && has_image_extension(&path) {
                out.push(path);
            }
        }
    }
    Ok(out)
}

/// `.png` / `.jpg` / `.jpeg` (case-insensitive) のいずれか。
/// `.tmp` 等は false。`.tmp` の最終的な reject は `ingest_photo` 側でも行うが、
/// reconcile 段階で list から除いておく方が tx を汚さない。
fn has_image_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::open;
    use crate::log_watcher::FakeFsEventSource;
    use std::path::PathBuf;
    use tempfile::tempdir;

    /// fixture: 空の DB pool + photo dir 用 tempdir。
    async fn fresh_setup() -> (SqlitePool, tempfile::TempDir, tempfile::TempDir) {
        let db_dir = tempdir().unwrap();
        let pool = open(&db_dir.path().join("test.db")).await.unwrap();
        let photo_dir = tempdir().unwrap();
        (pool, db_dir, photo_dir)
    }

    fn jst() -> Tz {
        "Asia/Tokyo".parse().unwrap()
    }

    fn config() -> PhotoScannerConfig {
        PhotoScannerConfig { tz: jst() }
    }

    /// VRChat 命名規則を満たす画像ファイルを実 fs に作る。content は空 (parser は名前しか見ない)。
    fn write_photo(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"").unwrap();
        p
    }

    async fn count_photos(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM photo_records")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    // -- handle_event ----------------------------------------------------------

    #[tokio::test]
    async fn handle_event_path_changed_inserts_photo_record() {
        // Arrange
        let (pool, _db_dir, photo_dir) = fresh_setup().await;
        let path = write_photo(
            photo_dir.path(),
            "VRChat_2026-05-10_12-34-56.789_1920x1080.png",
        );
        let (source, _tx) = FakeFsEventSource::new();
        let mut actor = PhotoScannerActor::new(pool.clone(), source, config());

        // Act
        let outcome = actor
            .handle_event(FsEvent::PathChanged(path))
            .await
            .unwrap();

        // Assert
        match outcome {
            HandleOutcome::Ingest(IngestOutcome::Recorded { .. }) => {}
            other => panic!("expected Ingest(Recorded), got {other:?}"),
        }
        assert_eq!(count_photos(&pool).await, 1);
    }

    #[tokio::test]
    async fn handle_event_path_removed_does_not_touch_db() {
        let (pool, _db_dir, _photo_dir) = fresh_setup().await;
        let (source, _tx) = FakeFsEventSource::new();
        let mut actor = PhotoScannerActor::new(pool.clone(), source, config());

        let outcome = actor
            .handle_event(FsEvent::PathRemoved(PathBuf::from("anything.png")))
            .await
            .unwrap();

        assert_eq!(outcome, HandleOutcome::Removed);
        assert_eq!(count_photos(&pool).await, 0, "PathRemoved は noop");
    }

    #[tokio::test]
    async fn handle_event_queue_overflow_returns_overflow_ack() {
        let (pool, _db_dir, _photo_dir) = fresh_setup().await;
        let (source, _tx) = FakeFsEventSource::new();
        let mut actor = PhotoScannerActor::new(pool.clone(), source, config());

        let outcome = actor.handle_event(FsEvent::QueueOverflow).await.unwrap();

        assert_eq!(outcome, HandleOutcome::OverflowAck);
    }

    // -- run ------------------------------------------------------------------

    #[tokio::test]
    async fn run_drains_source_and_inserts_each_changed_path_once() {
        // Arrange: 3 件 send → drop tx → run() が drain して終了
        let (pool, _db_dir, photo_dir) = fresh_setup().await;
        let p1 = write_photo(photo_dir.path(), "VRChat_2026-05-10_12-00-00_1920x1080.png");
        let p2 = write_photo(photo_dir.path(), "VRChat_2026-05-10_13-00-00_1920x1080.png");
        let p3 = write_photo(photo_dir.path(), "VRChat_2026-05-10_14-00-00_1920x1080.png");
        let (source, tx) = FakeFsEventSource::new();
        tx.send(FsEvent::PathChanged(p1)).await.unwrap();
        tx.send(FsEvent::PathChanged(p2)).await.unwrap();
        tx.send(FsEvent::PathChanged(p3)).await.unwrap();
        drop(tx); // source.next() が None を返すようにする
        let actor = PhotoScannerActor::new(pool.clone(), source, config());

        // Act
        actor.run().await.unwrap();

        // Assert
        assert_eq!(count_photos(&pool).await, 3);
    }

    // -- reconcile ------------------------------------------------------------

    #[tokio::test]
    async fn reconcile_picks_up_existing_vrchat_photos_in_dir() {
        // Arrange: dir に 2 件、片方は VRChat 形式、もう片方は無関係
        let (pool, _db_dir, photo_dir) = fresh_setup().await;
        let _vrchat = write_photo(photo_dir.path(), "VRChat_2026-05-10_12-34-56_1920x1080.png");
        let _other = write_photo(photo_dir.path(), "Discord_screenshot.png");
        let (source, _tx) = FakeFsEventSource::new();
        let mut actor = PhotoScannerActor::new(pool.clone(), source, config());

        // Act
        let outcome = actor.reconcile(photo_dir.path()).await.unwrap();

        // Assert: 2 件 considered, VRChat だけ recorded, Discord は skipped
        assert_eq!(outcome.considered, 2);
        assert_eq!(outcome.recorded, 1);
        assert_eq!(outcome.skipped, 1);
        assert_eq!(count_photos(&pool).await, 1);
    }

    #[tokio::test]
    async fn reconcile_skips_non_image_extensions_before_opening_tx() {
        // Arrange: dir に .txt と .png を 1 件ずつ
        let (pool, _db_dir, photo_dir) = fresh_setup().await;
        let _txt = write_photo(photo_dir.path(), "notes.txt");
        let _png = write_photo(photo_dir.path(), "VRChat_2026-05-10_12-34-56_1920x1080.png");
        let (source, _tx) = FakeFsEventSource::new();
        let mut actor = PhotoScannerActor::new(pool.clone(), source, config());

        // Act
        let outcome = actor.reconcile(photo_dir.path()).await.unwrap();

        // Assert: .txt は extension filter で list から除外されているので considered=1
        assert_eq!(
            outcome.considered, 1,
            ".txt は image extension filter で除外"
        );
        assert_eq!(outcome.recorded, 1);
    }

    #[tokio::test]
    async fn reconcile_returns_zero_outcome_for_empty_dir() {
        let (pool, _db_dir, photo_dir) = fresh_setup().await;
        let (source, _tx) = FakeFsEventSource::new();
        let mut actor = PhotoScannerActor::new(pool.clone(), source, config());

        let outcome = actor.reconcile(photo_dir.path()).await.unwrap();

        assert_eq!(
            outcome,
            ReconcileOutcome {
                considered: 0,
                recorded: 0,
                skipped: 0
            }
        );
    }

    #[tokio::test]
    async fn reconcile_returns_zero_outcome_when_dir_does_not_exist() {
        // Arrange: 存在しない dir を渡す。ユーザーが photo_directory 未設定で起動した想定。
        let (pool, _db_dir, _photo_dir) = fresh_setup().await;
        let nonexistent = std::env::temp_dir().join("vrcwatchdog_definitely_nonexistent_xyz");
        let _ = std::fs::remove_dir_all(&nonexistent); // 念のため
        let (source, _tx) = FakeFsEventSource::new();
        let mut actor = PhotoScannerActor::new(pool.clone(), source, config());

        let outcome = actor.reconcile(&nonexistent).await.unwrap();

        assert_eq!(
            outcome.considered, 0,
            "存在しない dir では panic せず空 outcome を返す"
        );
    }

    #[tokio::test]
    async fn reconcile_walks_into_subdirectories_to_find_photos() {
        // VRChat デフォルトの `Pictures/VRChat/YYYY-MM/VRChat_*.png` 構造を再現する。
        // root 直下にファイルは置かず、月別サブディレクトリに配置する。
        // Arrange
        let (pool, _db_dir, photo_dir) = fresh_setup().await;
        let month = photo_dir.path().join("2025-05");
        std::fs::create_dir(&month).unwrap();
        let _ = write_photo(&month, "VRChat_2026-05-10_12-34-56_1920x1080.png");
        let (source, _tx) = FakeFsEventSource::new();
        let mut actor = PhotoScannerActor::new(pool.clone(), source, config());

        // Act
        let outcome = actor.reconcile(photo_dir.path()).await.unwrap();

        // Assert: 再帰スキャンでサブディレクトリ内の 1 件を拾う
        assert_eq!(outcome.considered, 1);
        assert_eq!(outcome.recorded, 1);
        assert_eq!(count_photos(&pool).await, 1);
    }

    #[tokio::test]
    async fn reconcile_skips_hidden_directories_like_dotthumbnails() {
        // `.thumbnails` のような隠しディレクトリは VRChat 以外のツール (Synology Photos /
        // KDE 等) が作るメタデータ。混入させないことを保証する。
        // Arrange
        let (pool, _db_dir, photo_dir) = fresh_setup().await;
        let hidden = photo_dir.path().join(".thumbnails");
        std::fs::create_dir(&hidden).unwrap();
        let _ = write_photo(&hidden, "VRChat_2026-05-10_12-34-56_1920x1080.png");
        let (source, _tx) = FakeFsEventSource::new();
        let mut actor = PhotoScannerActor::new(pool.clone(), source, config());

        // Act
        let outcome = actor.reconcile(photo_dir.path()).await.unwrap();

        // Assert: 隠し dir 配下は無視
        assert_eq!(
            outcome.considered, 0,
            "隠しディレクトリ配下は scan 対象外"
        );
        assert_eq!(count_photos(&pool).await, 0);
    }

    #[tokio::test]
    async fn reconcile_is_idempotent_when_called_twice_on_same_dir() {
        // Arrange
        let (pool, _db_dir, photo_dir) = fresh_setup().await;
        let _ = write_photo(photo_dir.path(), "VRChat_2026-05-10_12-34-56_1920x1080.png");
        let (source, _tx) = FakeFsEventSource::new();
        let mut actor = PhotoScannerActor::new(pool.clone(), source, config());

        // Act
        let _ = actor.reconcile(photo_dir.path()).await.unwrap();
        let second = actor.reconcile(photo_dir.path()).await.unwrap();

        // Assert: 2 回目も同じファイルが見えるが photo_records には 1 件のまま
        // (file_path UNIQUE で idempotent)。
        assert_eq!(second.considered, 1);
        assert_eq!(
            second.recorded, 1,
            "Recorded カウントは insert 試行回数 (実 row 数とは別)"
        );
        assert_eq!(count_photos(&pool).await, 1);
    }
}
