//! ファイルシステムイベント源の trait 抽象。
//!
//! 本番では [`notify`] crate ベースの実装、テストでは fake (mpsc 直入れ) を使い、
//! 「ファイル切替」「不完全行→次回完成」「重複/順不同」「truncate」「notify overflow」を
//! 決定的に再現できるようにする。

use std::path::PathBuf;

use async_trait::async_trait;
use tokio::sync::mpsc;

/// ログディレクトリで起きた最小限のイベント。
///
/// 実 `notify` のセマンティクスの差を吸収するため、本 trait の実装側は
/// `Modify` 系を 1 種類に正規化、`Create`/`Remove` も同型にまとめてよい。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsEvent {
    /// 新規ファイル発見、または既存ファイルへの追記/サイズ変化。
    /// 呼び出し側は path から最新 metadata を取り直し、必要なら ingest を再開する。
    PathChanged(PathBuf),
    /// path が消えた (rename / delete)。HashMap から該当エントリを除去する目安。
    PathRemoved(PathBuf),
    /// 内部キューが溢れた (notify overflow 等)。reconcile scan を強制実行する。
    QueueOverflow,
}

/// ファイルシステムイベント源。
///
/// 本番実装 ([`super::notify_source::NotifyEventSource`]) と
/// fake 実装 ([`FakeFsEventSource`]) を切り替え可能にする。
#[async_trait]
pub trait FsEventSource: Send + Sync {
    /// 次のイベントを 1 件待つ。`None` で source 終了。
    async fn next(&mut self) -> Option<FsEvent>;
}

/// テスト用 fake event source。
/// `tx` 側からテストが任意のイベントを送り込める。
pub struct FakeFsEventSource {
    rx: mpsc::Receiver<FsEvent>,
}

impl FakeFsEventSource {
    pub fn new() -> (Self, mpsc::Sender<FsEvent>) {
        let (tx, rx) = mpsc::channel(64);
        (Self { rx }, tx)
    }
}

#[async_trait]
impl FsEventSource for FakeFsEventSource {
    async fn next(&mut self) -> Option<FsEvent> {
        self.rx.recv().await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_source_delivers_events_in_order() {
        let (mut src, tx) = FakeFsEventSource::new();
        let p = PathBuf::from("/tmp/a.log");
        tx.send(FsEvent::PathChanged(p.clone())).await.unwrap();
        tx.send(FsEvent::QueueOverflow).await.unwrap();
        drop(tx);

        assert_eq!(src.next().await, Some(FsEvent::PathChanged(p)));
        assert_eq!(src.next().await, Some(FsEvent::QueueOverflow));
        assert_eq!(src.next().await, None);
    }
}
