//! `notify-debouncer-mini` ベースの本番 photo `FsEventSource` 実装。
//!
//! plan §3 の指定通り **500ms debounce**。VRChat は screenshot 1 枚を細切れに
//! 書き込むことがあり、生 notify を流すと同じファイルに対して create + modify が
//! 連発する。debouncer がそれらを吸収して最後の状態だけを 1 イベントに集約する。
//!
//! mini の `DebouncedEventKind` は `Any` 一種類しかない (plan の意図とも一致:
//! actor 側は idempotent にしてあるので create/modify を区別する必要がない)。
//! ファイル削除も Any として通知されるが、`PhotoScannerActor::handle_event` の
//! `PathChanged` arm は filename parser → photo_records::insert と進むので
//! 「無くなったファイル」については ingest_photo 内の `path.file_name()` 抽出と
//! 拡張子チェックで自然に skip される (副作用なし)。

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, DebouncedEventKind, Debouncer};
use tokio::sync::mpsc;

use crate::log_watcher::{FsEvent, FsEventSource};
use crate::Result;

/// plan §3: 500ms debounce。
const DEBOUNCE_INTERVAL: Duration = Duration::from_millis(500);

pub struct NotifyPhotoSource {
    rx: mpsc::Receiver<FsEvent>,
    /// drop で内部 watcher も停止する。直接触らないが drop 順序保証のため保持。
    _debouncer: Debouncer<RecommendedWatcher>,
}

impl NotifyPhotoSource {
    /// 指定 `dir` を **再帰 watch** して、debounced 後のファイル変化を `FsEvent` 列に
    /// 流すソースを返す。
    ///
    /// VRChat デフォルトでは `Pictures/VRChat/YYYY-MM/` のように月別サブディレクトリに
    /// 写真を保存するため、root だけ watch しても新規撮影が拾えない。`Recursive` は
    /// notify が新規サブ dir も自動で watch 対象に加えるので、月またぎでも追従する。
    pub fn new(dir: &Path) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<FsEvent>(256);
        let tx_for_cb = tx.clone();
        let mut debouncer = new_debouncer(DEBOUNCE_INTERVAL, move |res: DebounceEventResult| {
            match res {
                Ok(events) => {
                    for ev in events {
                        // mini は Any のみ。ファイル変化として一括 PathChanged に正規化。
                        if matches!(ev.kind, DebouncedEventKind::Any) {
                            // notify thread は tokio runtime 外で回るので blocking_send。
                            let _ = tx_for_cb.blocking_send(FsEvent::PathChanged(ev.path));
                        }
                    }
                }
                Err(_errors) => {
                    // 1 件以上の error → notify バックエンドが過負荷状態の可能性。
                    // overflow とみなして actor に reconcile を促す。
                    let _ = tx_for_cb.blocking_send(FsEvent::QueueOverflow);
                }
            }
        })
        .map_err(|e| crate::Error::Config(format!("notify-debouncer-mini init: {e}")))?;

        debouncer
            .watcher()
            .watch(dir, RecursiveMode::Recursive)
            .map_err(|e| crate::Error::Config(format!("watcher.watch({}): {e}", dir.display())))?;

        // 元の sender は不要 (callback が clone を保持)。drop しないと receiver が
        // 早期に None を返してしまうので、ここで明示 drop。
        drop(tx);

        Ok(Self {
            rx,
            _debouncer: debouncer,
        })
    }
}

#[async_trait]
impl FsEventSource for NotifyPhotoSource {
    async fn next(&mut self) -> Option<FsEvent> {
        self.rx.recv().await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::io::AsyncWriteExt;

    /// real fs を使う smoke test。OS の inotify / FileSystemWatcher が
    /// debouncer 経由で actor 側に届くまでの繋ぎを確認する。
    ///
    /// 500ms debounce + α の遅延を見越して 2.5s で待つ。CI の遅さに耐えるよう余裕大きめ。
    #[tokio::test]
    async fn notify_photo_source_observes_file_creation_after_debounce() {
        // Arrange
        let dir = tempdir().unwrap();
        let mut src = NotifyPhotoSource::new(dir.path()).unwrap();

        // Act: ファイル作成 → debounce 経由でイベント到着
        let path = dir.path().join("test.png");
        let mut f = tokio::fs::File::create(&path).await.unwrap();
        f.write_all(b"VRChat photo bytes").await.unwrap();
        f.flush().await.unwrap();
        drop(f);

        // Assert: 2.5s 以内に少なくとも 1 イベント来る
        let event = tokio::time::timeout(Duration::from_millis(2500), src.next())
            .await
            .ok()
            .flatten();
        assert!(
            event.is_some(),
            "debounced FsEvent should arrive within 2.5s of file creation"
        );
    }
}
