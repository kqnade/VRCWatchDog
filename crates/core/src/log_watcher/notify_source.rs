//! `notify` crate ベースの本番 `FsEventSource` 実装。
//!
//! 単純な policy:
//! - `Create` / `Modify` (any subkind) → `FsEvent::PathChanged`
//! - `Remove` → `FsEvent::PathRemoved`
//! - その他 (Access / Other) → 無視
//! - `notify::Error` → `FsEvent::QueueOverflow` (reconcile 強制)
//!
//! `notify` のセマンティクスは OS 毎に微妙に違うが、actor 側は重複・順不同・欠落を
//! 前提に動作するので、ここで正規化を頑張る必要はない。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use super::fs_event::{FsEvent, FsEventSource};
use crate::Result;

pub struct NotifyEventSource {
    rx: mpsc::Receiver<FsEvent>,
    /// drop で watcher が停止する。直接触らないが drop 順序保証のため保持。
    _watcher: RecommendedWatcher,
}

impl NotifyEventSource {
    pub fn new(dir: &Path) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<FsEvent>(256);
        let tx_for_cb = tx.clone();
        let mut watcher: RecommendedWatcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                match res {
                    Ok(ev) => {
                        let event_kind = ev.kind;
                        let mapped = match event_kind {
                            EventKind::Create(_) | EventKind::Modify(_) => Some(false),
                            EventKind::Remove(_) => Some(true),
                            _ => None,
                        };
                        if let Some(is_remove) = mapped {
                            for path in ev.paths {
                                let event = if is_remove {
                                    FsEvent::PathRemoved(path)
                                } else {
                                    FsEvent::PathChanged(path)
                                };
                                // notify thread はランタイム外でも回るので blocking_send で OK
                                let _ = tx_for_cb.blocking_send(event);
                            }
                        }
                    }
                    Err(_e) => {
                        let _ = tx_for_cb.blocking_send(FsEvent::QueueOverflow);
                    }
                }
            })?;
        watcher.watch(dir, RecursiveMode::NonRecursive)?;
        drop(tx); // tx は callback 内 clone のみ保持、main 側 sender は不要
        Ok(Self {
            rx,
            _watcher: watcher,
        })
    }
}

#[async_trait]
impl FsEventSource for NotifyEventSource {
    async fn next(&mut self) -> Option<FsEvent> {
        self.rx.recv().await
    }
}

/// `RealFsProbe`: 実 fs に対する [`super::FsProbe`] 実装。
///
/// Linux: dev/inode を使った FileIdentity (Windows ではないので proxy)。
/// Windows: 別途 `windows` crate で `GetFileInformationByHandle` を呼ぶ実装が必要
/// (Phase 4a.6 で cfg(windows) として追加予定)。
pub struct RealFsProbe;

impl RealFsProbe {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RealFsProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl super::FsProbe for RealFsProbe {
    async fn probe(&self, path: &Path) -> Result<Option<super::ProbedFile>> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || probe_blocking(&path))
            .await
            .map_err(|e| crate::Error::Config(format!("spawn_blocking: {e}")))?
    }

    async fn read_from(&self, path: &Path, from: u64) -> Result<Vec<u8>> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
        let mut f = match tokio::fs::File::open(path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        f.seek(SeekFrom::Start(from)).await?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).await?;
        Ok(buf)
    }

    async fn list_dir(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            // ファイル拡張子 .txt のみ対象 (VRChat ログ命名)
            if path.extension().and_then(|s| s.to_str()) == Some("txt") {
                out.push(path);
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(unix)]
fn probe_blocking(path: &Path) -> Result<Option<super::ProbedFile>> {
    use std::os::unix::fs::MetadataExt;
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let dev = meta.dev();
    let inode = meta.ino();
    let mtime = meta.mtime();
    let creation_time = meta.ctime(); // Linux に birthtime はないので ctime で代替
    let bytes = read_head(path)?;
    let first_kb_hash = super::actor::compute_head_line_hash(&bytes);
    Ok(Some(super::ProbedFile {
        size: meta.len() as i64,
        first_kb_hash,
        creation_time,
        mtime_unix_secs: mtime,
        volume_serial: dev as u32,
        file_id_high: (inode >> 32) as u32,
        file_id_low: inode as u32,
    }))
}

#[cfg(windows)]
fn probe_blocking(path: &Path) -> Result<Option<super::ProbedFile>> {
    use std::os::windows::fs::MetadataExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let meta = file.metadata()?;
    // windows crate 0.58+: HANDLE(pub *mut c_void)。
    // std::os::windows::io::AsRawHandle::as_raw_handle() は RawHandle (= *mut c_void) を返すので
    // そのまま渡せる (as キャストは clippy::unnecessary_cast 違反になる)。
    let handle = HANDLE(file.as_raw_handle());
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: handle は直前で得た有効なファイルハンドル、info は valid な &mut。
    // FFI には *mut を明示してから渡す。
    let info_ptr: *mut BY_HANDLE_FILE_INFORMATION = &mut info;
    unsafe { GetFileInformationByHandle(handle, info_ptr) }
        .map_err(|e| crate::Error::Config(format!("GetFileInformationByHandle: {e}")))?;
    let creation_time = ((info.ftCreationTime.dwHighDateTime as i64) << 32)
        | (info.ftCreationTime.dwLowDateTime as i64);
    let bytes = read_head(path)?;
    let first_kb_hash = super::actor::compute_head_line_hash(&bytes);
    Ok(Some(super::ProbedFile {
        size: meta.file_size() as i64,
        first_kb_hash,
        creation_time,
        mtime_unix_secs: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        volume_serial: info.dwVolumeSerialNumber,
        file_id_high: info.nFileIndexHigh,
        file_id_low: info.nFileIndexLow,
    }))
}

/// 先頭 4KB だけ読む (head-line hash 計算用)。改行が見つからない巨大ファイルでも
/// 最大 4KB で打ち切る。
fn read_head(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut buf = vec![0u8; 4096];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::FsProbe;
    use super::*;
    use tempfile::tempdir;
    use tokio::io::AsyncWriteExt;

    /// smoke test: 実 notify と実 fs を組み合わせて、ファイル変更が actor 側に届くことを確認。
    /// Linux/Windows 両 OS で動く範囲のみ。
    #[tokio::test]
    async fn notify_source_observes_file_creation_and_modify() {
        let dir = tempdir().unwrap();
        let mut src = NotifyEventSource::new(dir.path()).unwrap();

        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, "hello\n").await.unwrap();

        // Linux inotify は数十 ms オーダーで届く。1 秒の timeout を取る。
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), src.next())
            .await
            .ok()
            .flatten();
        assert!(event.is_some(), "expected an FsEvent within 1s");

        // 追記でも検出されること
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap();
        f.write_all(b"world\n").await.unwrap();
        f.flush().await.unwrap();
        drop(f);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), src.next()).await;
        // 厳密な assertion はしない (notify の重複・順不同があり得るため)。
        // 重要なのは receiver が drain しても deadlock しないこと。
    }

    #[tokio::test]
    async fn real_fs_probe_returns_size_hash_and_ids() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        tokio::fs::write(&path, b"VRChat Build 2026.5.9\nmore\n")
            .await
            .unwrap();

        let probe = RealFsProbe::new();
        let probed = probe.probe(&path).await.unwrap().unwrap();
        assert!(probed.size >= 27);
        assert_ne!(probed.first_kb_hash, [0u8; 32]);
        assert!(probed.volume_serial != 0 || probed.file_id_low != 0);
    }

    #[tokio::test]
    async fn real_fs_probe_read_from_returns_remaining_bytes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let body = b"abcdefghij";
        tokio::fs::write(&path, body).await.unwrap();

        let probe = RealFsProbe::new();
        let bytes = probe.read_from(&path, 3).await.unwrap();
        assert_eq!(bytes, b"defghij");
    }

    #[tokio::test]
    async fn real_fs_probe_list_dir_filters_to_txt() {
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("a.txt"), b"x")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.log"), b"x")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("c.txt"), b"x")
            .await
            .unwrap();

        let probe = RealFsProbe::new();
        let files = probe.list_dir(dir.path()).await.unwrap();
        let names: Vec<String> = files
            .iter()
            .filter_map(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .collect();
        assert_eq!(names, vec!["a.txt", "c.txt"]);
    }
}
