//! Phase 4b 動作確認用 CLI。
//!
//! 既存 DB の `raw_log_events` の Pending を順次 projection する。
//! `cargo run -p vrcwatchdog-core --example project_raw -- [--db <PATH>]`

use std::path::Path;

use vrcwatchdog_core::db;
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
    let db_path = parse_arg(&args, "--db").unwrap_or_else(|| {
        std::env::temp_dir()
            .join("vrcwatchdog-ingest_raw.db")
            .to_string_lossy()
            .into_owned()
    });
    let pool = db::open(Path::new(&db_path)).await?;
    tracing::info!(db = %db_path, "opened pool");

    loop {
        let r = project_batch(&pool, 500).await?;
        if r.processed > 0 {
            tracing::info!(
                processed = r.processed,
                done = r.done,
                skipped = r.skipped,
                failed = r.failed,
                "projected"
            );
        }
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("ctrl_c received, shutting down");
                break;
            }
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
