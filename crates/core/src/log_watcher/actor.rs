//! [`LogWatcherActor`]: ingest 専用シーケンシャル actor。
//!
//! 役割:
//! 1. `FsEventSource` から file change イベントを受け取る
//! 2. `FileIdentity` を取得して generation bump 判定 ([`super::detect_bump`])
//! 3. `LineBuffer` で完了行のみ抽出
//! 4. `parse_line` で `LogEvent` に変換 (失敗時は `UnparsableLine` で raw に残す)
//! 5. 100 行 or 200ms whichever-first のバッチで
//!    [`crate::db::repo::raw_log::insert_batch_with_ledger`] に渡し、
//!    `ingest_position` を同一 tx で更新
//! 6. `Backpressure` (Pending ledger 行数) を監視し、上限超過で ingest を停止
//!
//! テストでは `FsProbe` trait と `FakeFsProbe` で「ファイルから何バイト読めるか」と
//! 「FileIdentity」をモック化し、実 fs を使わず crash/race を決定的に再現する。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::file_identity::{
    detect_bump, CurrentFileState, FileIdentity, GenerationBumpReason, PrevFileState,
};
use super::fs_event::{FsEvent, FsEventSource};
use super::line_buffer::LineBuffer;
use crate::db::repo::processed_log_files::{self, ProcessedLogFileInput};
use crate::db::repo::raw_log::{self, RawEventInput};
use crate::db::Pool;
use crate::log_parser::{derive_log_sequence_key, parse_line, LogEvent};
use crate::Result;

/// 1 ファイルの ingest 状態 (メモリキャッシュ。真の状態は `processed_log_files` 行)。
struct FileState {
    pf_id: i64,
    identity: FileIdentity,
    buffer: LineBuffer,
    /// `processed_log_files.ingest_position` のメモリ複製。
    cursor: u64,
}

/// ファイルからメタデータ (size / first_kb / creation_time / mtime) と
/// 「指定 offset 以降のバイト」を取得する抽象。
///
/// 本番は実 fs を読む実装、テストでは fake で挙動を制御する。
#[async_trait]
pub trait FsProbe: Send + Sync {
    /// 現在のファイル状態 (size, first_kb_hash, creation_time, mtime, volume_serial,
    /// file_id_high, file_id_low) を取得する。ファイルが存在しなければ `None`。
    async fn probe(&self, path: &Path) -> Result<Option<ProbedFile>>;

    /// `path` の `from..` バイトを読んで返す。EOF 到達まで読む。
    async fn read_from(&self, path: &Path, from: u64) -> Result<Vec<u8>>;

    /// 指定ディレクトリ直下のログファイル候補を列挙する。
    /// reconcile scan で「notify が見落としたファイル」を拾うために使う。
    async fn list_dir(&self, dir: &Path) -> Result<Vec<PathBuf>>;
}

#[derive(Debug, Clone)]
pub struct ProbedFile {
    pub size: i64,
    pub first_kb_hash: [u8; 32],
    pub creation_time: i64,
    pub mtime_unix_secs: i64,
    pub volume_serial: u32,
    pub file_id_high: u32,
    pub file_id_low: u32,
}

/// LogWatcher actor の動作設定。
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    pub batch_size: usize,
    pub batch_timeout: Duration,
    pub backlog_hard_limit: i64,
    pub tz_id: String,
    pub tz_source: String,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            batch_timeout: Duration::from_millis(200),
            backlog_hard_limit: 50_000,
            tz_id: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string()),
            tz_source: "CapturedRealtime".to_string(),
        }
    }
}

pub struct LogWatcherActor<S: FsEventSource, P: FsProbe> {
    pool: Pool,
    source: S,
    probe: Arc<P>,
    config: WatcherConfig,
    files: HashMap<PathBuf, FileState>,
    /// reconcile が走っている間 true。多重実行ガード (Phase 4a.4 で使用)。
    #[allow(dead_code)]
    reconcile_running: Arc<Mutex<bool>>,
}

impl<S: FsEventSource, P: FsProbe> LogWatcherActor<S, P> {
    pub fn new(pool: Pool, source: S, probe: Arc<P>, config: WatcherConfig) -> Self {
        Self {
            pool,
            source,
            probe,
            config,
            files: HashMap::new(),
            reconcile_running: Arc::new(Mutex::new(false)),
        }
    }

    /// 1 イベントを処理する。テスト・実運用どちらでも 1 イベントずつ呼べる
    /// 単純なエントリポイント。`run()` はこれをループする。
    pub async fn handle_event(&mut self, event: FsEvent) -> Result<HandleOutcome> {
        match event {
            FsEvent::PathChanged(path) => self.ingest_path(path).await,
            FsEvent::PathRemoved(path) => {
                self.files.remove(&path);
                Ok(HandleOutcome::Removed)
            }
            FsEvent::QueueOverflow => Ok(HandleOutcome::OverflowAck),
        }
    }

    /// メインループ。`source` が drain されるか hard limit 突入で終わる。
    pub async fn run(mut self) -> Result<()> {
        while let Some(ev) = self.source.next().await {
            let _ = self.handle_event(ev).await?;
        }
        Ok(())
    }

    /// reconcile scan: ディレクトリ内の全ログファイルに対して `ingest_path` を呼び、
    /// notify が見落としたファイル / 通知漏れのサイズ進捗を catch-up する。
    ///
    /// 多重実行ガードあり: 既に走行中なら [`ReconcileOutcome::AlreadyRunning`] を返す。
    pub async fn reconcile(&mut self, dir: &Path) -> Result<ReconcileOutcome> {
        // 多重実行ガード: 走行中フラグを取って確認 → 立てる → ロック解放
        // ロックを保持したまま `ingest_path` を呼ぶと再帰デッドロックの恐れがあるので、
        // フラグだけ立ててロックは即座に解放する。
        {
            let mut guard = self.reconcile_running.lock().await;
            if *guard {
                debug!("reconcile already running, skipping");
                return Ok(ReconcileOutcome::AlreadyRunning);
            }
            *guard = true;
        }

        // 本処理 (panic safety: catch_unwind は使わない、actor 親が supervise する)
        let result = self.reconcile_inner(dir).await;

        // フラグを必ず戻す
        {
            let mut guard = self.reconcile_running.lock().await;
            *guard = false;
        }

        result
    }

    async fn reconcile_inner(&mut self, dir: &Path) -> Result<ReconcileOutcome> {
        let paths = self.probe.list_dir(dir).await?;
        let mut catched_up: usize = 0;
        let mut skipped: usize = 0;
        for path in paths {
            match self.ingest_path(path).await? {
                HandleOutcome::Ingested { .. } | HandleOutcome::Buffered { .. } => {
                    catched_up += 1;
                }
                HandleOutcome::NoOp
                | HandleOutcome::Removed
                | HandleOutcome::OverflowAck
                | HandleOutcome::Paused { .. } => {
                    skipped += 1;
                }
            }
        }
        info!(catched_up, skipped, ?dir, "reconcile scan completed");
        Ok(ReconcileOutcome::Completed {
            catched_up,
            skipped,
        })
    }

    async fn ingest_path(&mut self, path: PathBuf) -> Result<HandleOutcome> {
        // 1. backpressure: backlog 上限を超えたら何もせず ack
        let backlog = self.current_backlog().await?;
        if backlog >= self.config.backlog_hard_limit {
            warn!(
                backlog,
                limit = self.config.backlog_hard_limit,
                "ingest paused"
            );
            return Ok(HandleOutcome::Paused { backlog });
        }

        // 2. 現在のファイル状態を probe
        let Some(probed) = self.probe.probe(&path).await? else {
            self.files.remove(&path);
            return Ok(HandleOutcome::Removed);
        };

        // 3. 既存メモリ state があれば generation bump を検知
        let (state_action, identity) = if let Some(prev) = self.files.get(&path) {
            let bump = detect_bump(
                PrevFileState {
                    generation: prev.identity.generation,
                    first_kb_hash: &prev.identity.first_kb_hash,
                    creation_time: prev.identity.creation_time,
                    mtime_unix_secs: 0, // メモリ state には mtime 持たないため 0 (creation/hash で十分)
                    ingest_position: prev.cursor as i64,
                },
                CurrentFileState {
                    size: probed.size,
                    first_kb_hash: &probed.first_kb_hash,
                    creation_time: probed.creation_time,
                    mtime_unix_secs: probed.mtime_unix_secs,
                },
            );
            if let Some((new_gen, reason)) = bump {
                debug!(?reason, ?path, new_gen, "file generation bumped");
                let new_id = FileIdentity {
                    volume_serial: probed.volume_serial,
                    file_id_high: probed.file_id_high,
                    file_id_low: probed.file_id_low,
                    generation: new_gen,
                    creation_time: probed.creation_time,
                    first_kb_hash: probed.first_kb_hash,
                };
                (StateAction::ResetForBump(reason), new_id)
            } else {
                (StateAction::ContinueExisting, prev.identity)
            }
        } else {
            // 初回登録: generation 0 から開始
            let new_id = FileIdentity {
                volume_serial: probed.volume_serial,
                file_id_high: probed.file_id_high,
                file_id_low: probed.file_id_low,
                generation: 0,
                creation_time: probed.creation_time,
                first_kb_hash: probed.first_kb_hash,
            };
            (StateAction::FirstSeen, new_id)
        };

        // 4. `processed_log_files` を upsert して pf_id を取得
        let pf_input = ProcessedLogFileInput {
            file_identity_hash: identity.identity_hash_hex(),
            log_sequence_key: derive_log_sequence_key(&path, Utc::now()),
            volume_serial: identity.volume_serial,
            file_id_high: identity.file_id_high,
            file_id_low: identity.file_id_low,
            generation: identity.generation,
            creation_time: identity.creation_time,
            first_kb_hash: hex_encode(&identity.first_kb_hash),
            file_name: path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string(),
            file_size: probed.size,
            mtime: chrono::DateTime::<Utc>::from_timestamp(probed.mtime_unix_secs, 0)
                .unwrap_or_else(Utc::now),
            tz_id: self.config.tz_id.clone(),
            tz_source: self.config.tz_source.clone(),
        };
        let mut tx = self.pool.begin().await?;
        let pf_id = processed_log_files::upsert(&mut tx, &pf_input).await?;
        tx.commit().await?;

        // 5. メモリ state を準備 (bump or 初回はカーソル 0 から)
        let cursor = match state_action {
            StateAction::FirstSeen | StateAction::ResetForBump(_) => {
                let mut buffer = LineBuffer::new();
                buffer.skip_utf8_bom();
                self.files.insert(
                    path.clone(),
                    FileState {
                        pf_id,
                        identity,
                        buffer,
                        cursor: 0,
                    },
                );
                0
            }
            StateAction::ContinueExisting => {
                let st = self
                    .files
                    .get(&path)
                    .expect("ContinueExisting implies entry exists");
                st.cursor
            }
        };

        // 6. cursor 以降を読んで LineBuffer に流し込む
        let new_bytes = self.probe.read_from(&path, cursor).await?;
        if new_bytes.is_empty() {
            return Ok(HandleOutcome::NoOp);
        }
        let state = self.files.get_mut(&path).expect("inserted above");
        state.buffer.extend_from_slice(&new_bytes);

        // 7. 完了行を取り出して raw + ledger insert + cursor 更新を同一 tx で
        let cursor_before = state.cursor;
        let lines = state.buffer.take_completed_lines();
        let line_count = lines.len();
        if lines.is_empty() {
            return Ok(HandleOutcome::Buffered {
                pending: state.buffer.pending_bytes(),
            });
        }
        let new_cursor = lines.last().map(|(_, off)| *off).unwrap_or(cursor_before);

        // 各 raw_log_events の byte_offset = その行の先頭バイト位置 (絶対値)。
        // 1 行目の先頭は cursor_before、2 行目以降は前行の end_after_lf。
        let mut prev_end = cursor_before;
        let inputs: Vec<RawEventInput> = lines
            .iter()
            .map(|(line, end)| {
                let start = prev_end as i64;
                prev_end = *end;
                let parsed = parse_line(line);
                let (event, naive_local) = match parsed {
                    Some(p) => (p.event, Some(p.naive_local)),
                    None => (
                        LogEvent::UnparsableLine {
                            reason: "no_timestamp_or_unparsable".to_string(),
                        },
                        None,
                    ),
                };
                RawEventInput {
                    processed_log_file_id: state.pf_id,
                    byte_offset: start,
                    event,
                    naive_local,
                }
            })
            .collect();

        let mut tx = self.pool.begin().await?;
        raw_log::insert_batch_with_ledger(&mut tx, &inputs).await?;
        processed_log_files::set_ingest_position(&mut tx, state.pf_id, new_cursor as i64).await?;
        tx.commit().await?;
        state.cursor = new_cursor;

        info!(?path, lines = line_count, cursor = new_cursor, "ingested");
        Ok(HandleOutcome::Ingested {
            lines: line_count,
            new_cursor,
        })
    }

    async fn current_backlog(&self) -> Result<i64> {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM projected_raw_events WHERE status = 'Pending'")
                .fetch_one(&self.pool)
                .await?;
        Ok(row.0)
    }
}

#[derive(Debug)]
#[allow(dead_code)] // ResetForBump の reason は Debug 経由で tracing 出力に使う
enum StateAction {
    FirstSeen,
    ContinueExisting,
    ResetForBump(GenerationBumpReason),
}

/// `handle_event` の結果。診断・テスト用。
#[derive(Debug)]
pub enum HandleOutcome {
    Ingested { lines: usize, new_cursor: u64 },
    Buffered { pending: usize },
    NoOp,
    Removed,
    OverflowAck,
    Paused { backlog: i64 },
}

/// `reconcile` の結果。
#[derive(Debug, PartialEq, Eq)]
pub enum ReconcileOutcome {
    Completed { catched_up: usize, skipped: usize },
    AlreadyRunning,
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// `FileIdentity::first_kb_hash` の計算規則。
///
/// 名前は legacy だが、実態は **「ファイル先頭から最初の改行まで」の blake3**。
/// 追記で hash が変動しないようにこの規則を選んでいる。VRChat ログは冒頭に
/// 固定的なヘッダ行 (`VRChat Build x.y.z`) を持つので、先頭 1 行だけで
/// 「同一ファイル世代」の識別に十分。改行未到達 (size 小) のときは現バイト全部を hash。
#[allow(dead_code)] // Phase 4a.5 の実 NotifyEventSource probe 実装で利用予定
pub fn compute_head_line_hash(bytes: &[u8]) -> [u8; 32] {
    let head = match bytes.iter().position(|&b| b == b'\n') {
        Some(idx) => &bytes[..idx],
        None => bytes,
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update(head);
    let mut out = [0u8; 32];
    out.copy_from_slice(hasher.finalize().as_bytes());
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::open;
    use crate::log_watcher::fs_event::FakeFsEventSource;
    use sqlx::Row;
    use tempfile::tempdir;

    /// テスト用 in-memory ファイルシステム。
    /// `append` / `truncate` / `delete_recreate` でログファイルの状態遷移を再現する。
    struct FakeFsProbe {
        inner: Mutex<HashMap<PathBuf, FakeFile>>,
    }

    #[derive(Clone)]
    struct FakeFile {
        bytes: Vec<u8>,
        creation_time: i64,
        mtime_unix_secs: i64,
        volume_serial: u32,
        file_id_high: u32,
        file_id_low: u32,
    }

    impl FakeFsProbe {
        fn new() -> Self {
            Self {
                inner: Mutex::new(HashMap::new()),
            }
        }

        async fn create(&self, path: PathBuf, bytes: &[u8], creation_time: i64) {
            let mut inner = self.inner.lock().await;
            inner.insert(
                path,
                FakeFile {
                    bytes: bytes.to_vec(),
                    creation_time,
                    mtime_unix_secs: creation_time,
                    volume_serial: 0xCAFE,
                    file_id_high: 0,
                    file_id_low: 0xABCDEF01,
                },
            );
        }

        async fn append(&self, path: &Path, bytes: &[u8], mtime: i64) {
            let mut inner = self.inner.lock().await;
            let f = inner.get_mut(path).expect("file not created");
            f.bytes.extend_from_slice(bytes);
            f.mtime_unix_secs = mtime;
        }

        async fn truncate(&self, path: &Path, mtime: i64) {
            let mut inner = self.inner.lock().await;
            let f = inner.get_mut(path).expect("file not created");
            f.bytes.clear();
            f.mtime_unix_secs = mtime;
        }

        async fn delete_recreate(&self, path: &Path, bytes: &[u8], creation_time: i64) {
            let mut inner = self.inner.lock().await;
            let f = inner.get_mut(path).expect("file not created");
            f.bytes = bytes.to_vec();
            f.creation_time = creation_time;
            f.mtime_unix_secs = creation_time;
            // file_id を新規にして delete+recreate を表現
            f.file_id_low = f.file_id_low.wrapping_add(1);
        }
    }

    #[async_trait]
    impl FsProbe for FakeFsProbe {
        async fn probe(&self, path: &Path) -> Result<Option<ProbedFile>> {
            let inner = self.inner.lock().await;
            Ok(inner.get(path).map(|f| ProbedFile {
                size: f.bytes.len() as i64,
                first_kb_hash: super::compute_head_line_hash(&f.bytes),
                creation_time: f.creation_time,
                mtime_unix_secs: f.mtime_unix_secs,
                volume_serial: f.volume_serial,
                file_id_high: f.file_id_high,
                file_id_low: f.file_id_low,
            }))
        }

        async fn read_from(&self, path: &Path, from: u64) -> Result<Vec<u8>> {
            let inner = self.inner.lock().await;
            Ok(inner
                .get(path)
                .map(|f| {
                    let from = (from as usize).min(f.bytes.len());
                    f.bytes[from..].to_vec()
                })
                .unwrap_or_default())
        }

        async fn list_dir(&self, dir: &Path) -> Result<Vec<PathBuf>> {
            let inner = self.inner.lock().await;
            let mut out: Vec<PathBuf> = inner
                .keys()
                .filter(|p| p.parent() == Some(dir) || dir.as_os_str().is_empty())
                .cloned()
                .collect();
            out.sort();
            Ok(out)
        }
    }

    async fn make_pool() -> (Pool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let pool = open(&path).await.unwrap();
        (pool, dir)
    }

    fn make_actor(
        pool: Pool,
        probe: Arc<FakeFsProbe>,
    ) -> LogWatcherActor<FakeFsEventSource, FakeFsProbe> {
        let (source, _tx) = FakeFsEventSource::new();
        LogWatcherActor::new(pool, source, probe, WatcherConfig::default())
    }

    fn log_line(ts: &str, body: &str) -> String {
        format!("{ts} Log        -  {body}\n")
    }

    /// 不変条件: 1 行 ingest で raw_log_events 1 件 + ledger Pending 1 件。
    #[tokio::test]
    async fn simple_ingest_writes_raw_and_advances_cursor() {
        let (pool, _dir) = make_pool().await;
        let probe = Arc::new(FakeFsProbe::new());
        let path = PathBuf::from("output_log_2026-05-09_21-43-56.txt");
        let line = log_line("2026.05.09 21:43:56", "[Behaviour] Entering Room: Alpha");
        probe.create(path.clone(), line.as_bytes(), 100).await;

        let mut actor = make_actor(pool.clone(), probe.clone());
        let outcome = actor
            .handle_event(FsEvent::PathChanged(path.clone()))
            .await
            .unwrap();
        assert!(matches!(outcome, HandleOutcome::Ingested { lines: 1, .. }));

        let raw_count: i64 = sqlx::query("SELECT COUNT(*) FROM raw_log_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        let ledger_count: i64 =
            sqlx::query("SELECT COUNT(*) FROM projected_raw_events WHERE status = 'Pending'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get(0);
        assert_eq!(raw_count, 1);
        assert_eq!(ledger_count, 1);

        let cursor: i64 = sqlx::query("SELECT ingest_position FROM processed_log_files")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(cursor, line.len() as i64);
    }

    /// 不変条件: 不完全行 (改行未到達) は raw に書かれず、次回 ingest で完成する。
    #[tokio::test]
    async fn incomplete_line_buffered_until_complete() {
        let (pool, _dir) = make_pool().await;
        let probe = Arc::new(FakeFsProbe::new());
        let path = PathBuf::from("output_log_2026-05-09_21-43-56.txt");
        // 改行なしで書き始め
        probe
            .create(
                path.clone(),
                b"2026.05.09 21:00:00 Log        -  [Behaviour] Entering Room: Alp",
                100,
            )
            .await;

        let mut actor = make_actor(pool.clone(), probe.clone());
        let outcome = actor
            .handle_event(FsEvent::PathChanged(path.clone()))
            .await
            .unwrap();
        assert!(matches!(outcome, HandleOutcome::Buffered { .. }));

        let raw_count: i64 = sqlx::query("SELECT COUNT(*) FROM raw_log_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(raw_count, 0, "incomplete line must not be persisted");

        // 続きが書かれて改行が来た
        probe.append(&path, b"ha\n", 200).await;
        let outcome = actor
            .handle_event(FsEvent::PathChanged(path.clone()))
            .await
            .unwrap();
        assert!(matches!(outcome, HandleOutcome::Ingested { lines: 1, .. }));

        let raw_count: i64 = sqlx::query("SELECT COUNT(*) FROM raw_log_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(raw_count, 1);
    }

    /// 不変条件: 同 actor で同じ path を 2 回 ingest しても raw が増えない (idempotency)。
    #[tokio::test]
    async fn duplicate_event_is_idempotent_within_actor() {
        let (pool, _dir) = make_pool().await;
        let probe = Arc::new(FakeFsProbe::new());
        let path = PathBuf::from("output_log_2026-05-09_21-43-56.txt");
        let line = log_line("2026.05.09 21:43:56", "[Behaviour] Entering Room: Alpha");
        probe.create(path.clone(), line.as_bytes(), 100).await;

        let mut actor = make_actor(pool.clone(), probe.clone());
        actor
            .handle_event(FsEvent::PathChanged(path.clone()))
            .await
            .unwrap();
        // 2 回目 (notify duplicate を模擬): cursor が進んでいるので read_from は空、NoOp。
        let outcome = actor
            .handle_event(FsEvent::PathChanged(path.clone()))
            .await
            .unwrap();
        assert!(matches!(outcome, HandleOutcome::NoOp));

        let raw_count: i64 = sqlx::query("SELECT COUNT(*) FROM raw_log_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(raw_count, 1);
    }

    /// 不変条件: actor 再起動 (= crash 模擬) しても、cursor が DB に永続化されているため
    /// 同 raw を再 insert しない。再起動後に追記された行のみ取り込む。
    #[tokio::test]
    async fn restart_after_crash_does_not_duplicate_persisted_raw() {
        let (pool, _dir) = make_pool().await;
        let probe = Arc::new(FakeFsProbe::new());
        let path = PathBuf::from("output_log_2026-05-09_21-43-56.txt");
        let line1 = log_line("2026.05.09 21:00:00", "[Behaviour] Entering Room: A");
        probe.create(path.clone(), line1.as_bytes(), 100).await;

        // 1 回目の actor: line1 を ingest
        {
            let mut actor = make_actor(pool.clone(), probe.clone());
            actor
                .handle_event(FsEvent::PathChanged(path.clone()))
                .await
                .unwrap();
        } // actor drop = crash 模擬

        // 続きを追記
        let line2 = log_line("2026.05.09 21:01:00", "[Behaviour] Entering Room: B");
        probe.append(&path, line2.as_bytes(), 200).await;

        // 新規 actor で再開: cursor は DB から復元される (メモリ HashMap は空)。
        // しかし actor は path を再度 probe するときに既存の processed_log_files から
        // ingest_position を取得しないと cursor=0 から始めてしまう。これは Phase 4a.4
        // で reconcile に組み込むので、ここでは PathChanged 経由の simple ingest として
        // memory state を保持した actor (= 1 回目から続行) で確認する。
        //
        // Crash 模擬で重要なのは「DB レベルで重複が起きないこと」。新 actor で cursor=0
        // から ingest しても UNIQUE 制約が守る。
        {
            let mut actor = make_actor(pool.clone(), probe.clone());
            actor
                .handle_event(FsEvent::PathChanged(path.clone()))
                .await
                .unwrap();
        }

        let rows: Vec<(i64, i64, String, String)> = sqlx::query_as(
            "SELECT id, byte_offset, event_type, payload_json FROM raw_log_events ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let raw_count = rows.len();
        // line1 + line2 = 2 件。重複 0。
        assert_eq!(
            raw_count, 2,
            "must have exactly 2 raw events (no duplicates from re-ingest). Got: {rows:?}"
        );
        let ledger_count: i64 = sqlx::query("SELECT COUNT(*) FROM projected_raw_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(ledger_count, 2);
    }

    /// 不変条件: truncate (size < cursor) で generation が bump し、新世代として ingest される。
    #[tokio::test]
    async fn truncate_bumps_generation_and_creates_new_pf_row() {
        let (pool, _dir) = make_pool().await;
        let probe = Arc::new(FakeFsProbe::new());
        let path = PathBuf::from("output_log_2026-05-09_21-43-56.txt");
        let line1 = log_line("2026.05.09 21:00:00", "[Behaviour] Entering Room: A");
        probe.create(path.clone(), line1.as_bytes(), 100).await;

        let mut actor = make_actor(pool.clone(), probe.clone());
        actor
            .handle_event(FsEvent::PathChanged(path.clone()))
            .await
            .unwrap();

        // Truncate して新規行を書く
        probe.truncate(&path, 200).await;
        let line2 = log_line("2026.05.09 21:30:00", "[Behaviour] Entering Room: B");
        probe.append(&path, line2.as_bytes(), 201).await;

        actor
            .handle_event(FsEvent::PathChanged(path.clone()))
            .await
            .unwrap();

        let pf_rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT generation, last_projected_raw_event_id FROM processed_log_files ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        // generation 0 と generation 1 の 2 行存在する
        let generations: Vec<i64> = pf_rows.iter().map(|(g, _)| *g).collect();
        assert!(
            generations.contains(&0) && generations.contains(&1),
            "expected generations to include 0 and 1, got {generations:?}"
        );
    }

    /// 不変条件: タイムスタンプの無い行 (パース失敗) は UnparsableLine として raw 永続化される。
    #[tokio::test]
    async fn unparsable_line_persisted_as_raw() {
        let (pool, _dir) = make_pool().await;
        let probe = Arc::new(FakeFsProbe::new());
        let path = PathBuf::from("output_log_2026-05-09_21-43-56.txt");
        // タイムスタンプ無し
        probe
            .create(path.clone(), b"VRChat Build 2026.5.9\n", 100)
            .await;

        let mut actor = make_actor(pool.clone(), probe.clone());
        actor
            .handle_event(FsEvent::PathChanged(path.clone()))
            .await
            .unwrap();

        let row: (String,) = sqlx::query_as("SELECT event_type FROM raw_log_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0, "UnparsableLine");
    }

    /// 不変条件: reconcile scan が notify 見落としファイルを catch-up する。
    /// notify が一切来ない状態で reconcile だけで履歴が完成することを確認。
    #[tokio::test]
    async fn reconcile_picks_up_missed_files() {
        let (pool, _dir) = make_pool().await;
        let probe = Arc::new(FakeFsProbe::new());
        let dir = PathBuf::from("/logs");
        let path1 = dir.join("output_log_2026-05-09_21-00-00.txt");
        let path2 = dir.join("output_log_2026-05-09_22-00-00.txt");
        let line1 = log_line("2026.05.09 21:00:00", "[Behaviour] Entering Room: A");
        let line2 = log_line("2026.05.09 22:00:00", "[Behaviour] Entering Room: B");
        probe.create(path1.clone(), line1.as_bytes(), 100).await;
        probe.create(path2.clone(), line2.as_bytes(), 200).await;

        let mut actor = make_actor(pool.clone(), probe.clone());
        // notify event は一切送らず、reconcile だけで catch-up
        let outcome = actor.reconcile(&dir).await.unwrap();
        match outcome {
            ReconcileOutcome::Completed { catched_up, .. } => {
                assert_eq!(catched_up, 2, "both files should be ingested");
            }
            _ => panic!("expected Completed, got {outcome:?}"),
        }

        let raw_count: i64 = sqlx::query("SELECT COUNT(*) FROM raw_log_events")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(raw_count, 2);
        let pf_count: i64 = sqlx::query("SELECT COUNT(*) FROM processed_log_files")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(pf_count, 2);
    }

    /// 不変条件: 同 reconcile 中に reconcile を再呼び出ししても多重実行されない。
    /// 多重 reconcile は file_size と cursor の race を起こすので必ず 1 つに絞る。
    #[tokio::test]
    async fn reconcile_guard_prevents_concurrent_runs() {
        let (pool, _dir) = make_pool().await;
        let probe = Arc::new(FakeFsProbe::new());
        let dir = PathBuf::from("/logs");
        let mut actor = make_actor(pool.clone(), probe.clone());

        // 走行中フラグを手動で立てる (実 reconcile を spawn する代わりに
        // 状態だけ模倣することで、テストを決定的に保つ)
        {
            let mut guard = actor.reconcile_running.lock().await;
            *guard = true;
        }
        let outcome = actor.reconcile(&dir).await.unwrap();
        assert_eq!(outcome, ReconcileOutcome::AlreadyRunning);

        // フラグを戻して再実行できることを確認
        {
            let mut guard = actor.reconcile_running.lock().await;
            *guard = false;
        }
        let outcome = actor.reconcile(&dir).await.unwrap();
        assert!(matches!(outcome, ReconcileOutcome::Completed { .. }));
    }

    /// 不変条件: 削除→再作成 (creation_time 変化) で generation が bump する。
    #[tokio::test]
    async fn delete_recreate_bumps_generation_via_creation_time() {
        let (pool, _dir) = make_pool().await;
        let probe = Arc::new(FakeFsProbe::new());
        let path = PathBuf::from("output_log_2026-05-09_21-43-56.txt");
        let line1 = log_line("2026.05.09 21:00:00", "[Behaviour] Entering Room: A");
        probe.create(path.clone(), line1.as_bytes(), 100).await;

        let mut actor = make_actor(pool.clone(), probe.clone());
        actor
            .handle_event(FsEvent::PathChanged(path.clone()))
            .await
            .unwrap();

        // Delete + recreate (creation_time が変わる)
        let line2 = log_line("2026.05.09 22:00:00", "[Behaviour] Entering Room: B");
        probe.delete_recreate(&path, line2.as_bytes(), 999).await;
        actor
            .handle_event(FsEvent::PathChanged(path.clone()))
            .await
            .unwrap();

        let pf_count: i64 = sqlx::query("SELECT COUNT(*) FROM processed_log_files")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(
            pf_count, 2,
            "delete+recreate must create a new processed_log_files row"
        );
    }
}
