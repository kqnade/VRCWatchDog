//! Phase 4a 動作確認用 CLI。
//!
//! 使用例:
//! ```sh
//! cargo run -p vrcwatchdog-core --example ingest_raw -- \
//!     --log-dir "$LOCALAPPDATA/../LocalLow/VRChat/VRChat" \
//!     --db /tmp/vrcwd-ingest.db
//! ```
//!
//! 内部で:
//! 1. DB を開く + マイグレーション実行
//! 2. NotifyEventSource + RealFsProbe で LogWatcherActor を構築
//! 3. 初回 reconcile で既存ファイルを catch-up
//! 4. notify ループで継続的に ingest
//! 5. Ctrl-C で graceful 終了

use std::path::{Path, PathBuf};
use std::sync::Arc;

use vrcwatchdog_core::db;
use vrcwatchdog_core::log_watcher::{
    LogWatcherActor, NotifyEventSource, RealFsProbe, WatcherConfig,
};
use vrcwatchdog_core::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let Some(log_dir) = parse_arg(&args, "--log-dir") else {
        eprintln!("usage: ingest_raw --log-dir <PATH> [--db <PATH>]");
        std::process::exit(1);
    };
    let db_path = parse_arg(&args, "--db").unwrap_or_else(|| {
        std::env::temp_dir()
            .join("vrcwatchdog-ingest_raw.db")
            .to_string_lossy()
            .into_owned()
    });

    let log_dir = PathBuf::from(log_dir);
    if !log_dir.is_dir() {
        eprintln!(
            "log dir not found or not a directory: {}",
            log_dir.display()
        );
        std::process::exit(1);
    }

    let pool = db::open(Path::new(&db_path)).await?;
    tracing::info!(db = %db_path, log_dir = %log_dir.display(), "opened pool");

    let probe = Arc::new(RealFsProbe::new());
    let source = NotifyEventSource::new(&log_dir)?;
    let mut actor = LogWatcherActor::new(
        pool.clone(),
        source,
        probe.clone(),
        WatcherConfig::default(),
    );

    // 初回 reconcile (notify が起動前にあったファイルも拾う)
    let outcome = actor.reconcile(&log_dir).await?;
    tracing::info!(?outcome, "initial reconcile done");

    // 周期 reconcile タスク (30 秒間隔)。actor とは別タスクで dir を直接監視。
    // ※ 本格的な統合は Phase 5 の app/main.rs で actor 自体を渡せるようにする予定。
    //   この example では beat だけ表示する。
    let log_dir_for_beat = log_dir.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.tick().await; // 即時 tick を消費
        loop {
            interval.tick().await;
            tracing::debug!(dir = %log_dir_for_beat.display(), "reconcile tick (placeholder)");
        }
    });

    // run loop と Ctrl-C を競争
    tokio::select! {
        res = actor.run() => {
            tracing::info!("event source drained");
            res?;
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("ctrl_c received, shutting down");
        }
    }
    Ok(())
}

fn parse_arg(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
