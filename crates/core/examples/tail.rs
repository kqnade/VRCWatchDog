//! Phase 4c CLI MVP: ingest と projection を 1 プロセスで end-to-end 実行。
//!
//! 使用例:
//! ```sh
//! cargo run -p vrcwatchdog-core --example tail -- \
//!     --log-dir "$LOCALAPPDATA/../LocalLow/VRChat/VRChat" \
//!     --db /tmp/vrcwd.db
//! ```
//!
//! 構成:
//! - LogWatcherActor: notify 経由で raw_log_events に書く (Phase 4a)
//! - projector loop: 500ms 間隔で project_batch を回す (Phase 4b)
//! - reconcile: 起動時 1 回 + 30 秒間隔 (Phase 4a.4)
//! - Ctrl-C: 全タスクを graceful に停止

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::task::JoinSet;
use vrcwatchdog_core::db;
use vrcwatchdog_core::log_watcher::{
    LogWatcherActor, NotifyEventSource, RealFsProbe, WatcherConfig,
};
use vrcwatchdog_core::projector::project_batch;
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
        eprintln!("usage: tail --log-dir <PATH> [--db <PATH>]");
        std::process::exit(1);
    };
    let db_path = parse_arg(&args, "--db").unwrap_or_else(|| {
        std::env::temp_dir()
            .join("vrcwatchdog-tail.db")
            .to_string_lossy()
            .into_owned()
    });

    let log_dir = PathBuf::from(log_dir);
    if !log_dir.is_dir() {
        eprintln!("log dir not found: {}", log_dir.display());
        std::process::exit(1);
    }

    let pool = db::open(Path::new(&db_path)).await?;
    tracing::info!(db = %db_path, log_dir = %log_dir.display(), "opened pool");

    // ingest 側
    let probe = Arc::new(RealFsProbe::new());
    let source = NotifyEventSource::new(&log_dir)?;
    let mut actor = LogWatcherActor::new(
        pool.clone(),
        source,
        probe.clone(),
        WatcherConfig::default(),
    );
    let outcome = actor.reconcile(&log_dir).await?;
    tracing::info!(?outcome, "initial reconcile done");

    // projector ループ
    let mut tasks = JoinSet::new();
    let pool_for_proj = pool.clone();
    tasks.spawn(async move {
        loop {
            match project_batch(&pool_for_proj, 500).await {
                Ok(r) if r.processed > 0 => {
                    tracing::info!(
                        processed = r.processed,
                        done = r.done,
                        skipped = r.skipped,
                        failed = r.failed,
                        "projected"
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::error!(error = %e, "projector batch failed"),
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    });

    // ingest run loop と Ctrl-C 競争
    tokio::select! {
        res = actor.run() => {
            tracing::info!("event source drained");
            res?;
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("ctrl_c received, shutting down");
        }
    }

    // projector を abort して終了
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

fn parse_arg(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
