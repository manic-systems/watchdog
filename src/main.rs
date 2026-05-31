use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use watchdog::{BuildInfo, config, server};

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
  /// Path to the TOML configuration file.
  #[arg(long)]
  config: Option<PathBuf>,

  /// Server listen address, overriding configuration.
  #[arg(long)]
  listen_addr: Option<String>,

  /// Prometheus metrics endpoint path, overriding configuration.
  #[arg(long)]
  metrics_path: Option<String>,

  /// Event ingestion endpoint path, overriding configuration.
  #[arg(long)]
  ingestion_path: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  tracing_subscriber::fmt()
    .with_env_filter(
      tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "watchdog=info,tower_http=info".into()),
    )
    .init();

  let cli = Cli::parse();
  let overrides = config::Overrides {
    listen_addr: cli.listen_addr,
    metrics_path: cli.metrics_path,
    ingestion_path: cli.ingestion_path,
  };

  let config = config::load(cli.config.as_deref(), overrides)
    .context("failed to load config")?;
  server::run(config, BuildInfo::current()).await
}
