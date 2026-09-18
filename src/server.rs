use std::net::SocketAddr;

use anyhow::Context;
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
  BuildInfo,
  app::{self, AppState},
  config::Config,
  limits::{SHUTDOWN_TIMEOUT, UNIQUES_UPDATE_PERIOD},
};

/// Runs the HTTP server until a shutdown signal is received.
pub async fn run(config: Config, build_info: BuildInfo) -> anyhow::Result<()> {
  info!(domains = ?config.site.domains, "loaded configuration");

  let state = AppState::new(config, build_info)
    .context("failed to initialize application")?;
  if let Err(err) = state.load_state().await {
    warn!(error = %err, "could not restore unique visitor state");
  }

  let app = app::router(state.clone()).context("failed to build router")?;
  let listen_addr: SocketAddr = state
    .config()
    .server
    .listen_addr
    .parse()
    .context("server.listen_addr must be a socket address")?;
  let listener = TcpListener::bind(listen_addr)
    .await
    .with_context(|| format!("failed to bind {listen_addr}"))?;
  let shutdown = CancellationToken::new();
  let uniques_task =
    spawn_unique_gauge_updater(state.clone(), shutdown.child_token());

  info!(addr = %listen_addr, metrics = %state.config().server.metrics_path, ingestion = %state.config().server.ingestion_path, "starting server");

  axum::serve(
    listener,
    app.into_make_service_with_connect_info::<SocketAddr>(),
  )
  .with_graceful_shutdown(shutdown_signal(shutdown.clone()))
  .await
  .context("server error")?;

  shutdown.cancel();
  if let Err(err) = tokio::time::timeout(SHUTDOWN_TIMEOUT, uniques_task).await {
    warn!(error = %err, "unique visitor gauge task did not stop before timeout");
  }

  if let Err(err) = state.save_state().await {
    error!(error = %err, "failed to persist unique visitor state");
  }

  info!("graceful shutdown complete");
  Ok(())
}

fn spawn_unique_gauge_updater(
  state: AppState,
  shutdown: CancellationToken,
) -> JoinHandle<()> {
  tokio::spawn(async move {
    let mut interval = tokio::time::interval(UNIQUES_UPDATE_PERIOD);
    loop {
      tokio::select! {
          _ = interval.tick() => state.metrics().update_unique_gauge(),
          _ = shutdown.cancelled() => break,
      }
    }
  })
}

async fn shutdown_signal(shutdown: CancellationToken) {
  #[cfg(unix)]
  {
    match tokio::signal::unix::signal(
      tokio::signal::unix::SignalKind::terminate(),
    ) {
      Ok(mut terminate) => {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
      },
      Err(err) => {
        warn!(error = %err, "could not install SIGTERM handler; waiting for Ctrl-C");
        let _ = tokio::signal::ctrl_c().await;
      },
    }
  }

  #[cfg(not(unix))]
  {
    let _ = tokio::signal::ctrl_c().await;
  }

  shutdown.cancel();
}
