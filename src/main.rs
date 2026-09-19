use std::{env::args_os, path::PathBuf, process::exit};

use anyhow::Context as _;
use pound::Parse;
use watchdog::{BuildInfo, config, server};

/// Command-line interface for the Watchdog server.
#[derive(Debug, Parse)]
struct Cli {
  /// Path to the TOML configuration file.
  #[pound(long)]
  config: Option<PathBuf>,

  /// Server listen address, overriding configuration.
  #[pound(long)]
  listen_addr: Option<String>,

  /// Prometheus metrics endpoint path, overriding configuration.
  #[pound(long)]
  metrics_path: Option<String>,

  /// Event ingestion endpoint path, overriding configuration.
  #[pound(long)]
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

  let arguments = args_os()
    .skip(1)
    .map(|argument| {
      #[expect(
        clippy::print_stderr,
        reason = "CLI must report invalid arguments before logging exists"
      )]
      argument.into_string().unwrap_or_else(|invalid| {
        eprintln!(
          "command-line arguments must be valid UTF-8, got {}",
          invalid.display()
        );
        exit(2);
      })
    })
    .collect::<Vec<_>>();

  let cli = Cli::parse_from(arguments.iter().map(String::as_str));
  let overrides = config::Overrides {
    listen_addr:    cli.listen_addr,
    metrics_path:   cli.metrics_path,
    ingestion_path: cli.ingestion_path,
  };

  let config = config::load(cli.config.as_deref(), overrides)
    .context("failed to load config")?;
  server::run(config, BuildInfo::current()).await
}
