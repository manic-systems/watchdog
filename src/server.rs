use std::net::SocketAddr;

use anyhow::Context as _;
use axum::{Extension, Router, extract::ConnectInfo, serve::Listener};
use hyper::server::conn::http1::Builder;
use hyper_util::{
  rt::{TokioIo, TokioTimer},
  service::TowerToHyperService,
};
#[cfg(unix)] use tokio::signal::unix::{SignalKind, signal};
use tokio::{
  net::{TcpListener, TcpStream},
  signal::ctrl_c,
  spawn,
  task::{JoinHandle, JoinSet},
  time::{interval, timeout},
};
use tokio_io_timeout::TimeoutStream;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::{
  BuildInfo,
  app::{self, AppState},
  config::Config,
  limits::{
    HTTP_READ_TIMEOUT,
    HTTP_WRITE_TIMEOUT,
    SHUTDOWN_TIMEOUT,
    UNIQUES_UPDATE_PERIOD,
  },
};

/// Runs the HTTP server until a shutdown signal is received.
///
/// # Errors
///
/// Returns an error when the listen address is invalid, the socket cannot be
/// bound, or persisted visitor state cannot be saved.
#[inline]
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
  let mut listener = TcpListener::bind(listen_addr)
    .await
    .with_context(|| format!("failed to bind {listen_addr}"))?;
  let shutdown = CancellationToken::new();
  let mut uniques_task =
    spawn_unique_gauge_updater(state.clone(), shutdown.child_token());
  let mut connections = JoinSet::new();
  let exit_signal = shutdown_signal(shutdown.clone());
  tokio::pin!(exit_signal);

  info!(addr = %listen_addr, metrics = %state.config().server.metrics_path, ingestion = %state.config().server.ingestion_path, "starting server");

  loop {
    tokio::select! {
      () = &mut exit_signal => break,
      (stream, remote_addr) = Listener::accept(&mut listener) => {
        connections.spawn(serve_connection(stream, remote_addr, app.clone(), shutdown.clone()));
      }
      Some(result) = connections.join_next(), if !connections.is_empty() => {
        if let Err(err) = result {
          warn!(error = %err, "HTTP connection task failed");
        }
      }
    }
  }

  drop(listener);
  shutdown.cancel();

  let drain = async {
    while let Some(result) = connections.join_next().await {
      if let Err(err) = result {
        warn!(error = %err, "HTTP connection task failed");
      }
    }
  };

  if timeout(SHUTDOWN_TIMEOUT, drain).await.is_err() {
    warn!("HTTP connections did not stop before timeout");
    connections.shutdown().await;
  }

  match timeout(SHUTDOWN_TIMEOUT, &mut uniques_task).await {
    Ok(Ok(())) => {},
    Ok(Err(err)) => warn!(error = %err, "unique visitor gauge task failed"),
    Err(err) => {
      uniques_task.abort();
      warn!(error = %err, "unique visitor gauge task did not stop before timeout");
    },
  }

  state
    .save_state()
    .await
    .context("failed to persist unique visitor state")?;

  info!("graceful shutdown complete");
  Ok(())
}

/// Serves one accepted TCP connection with read and write timeouts.
async fn serve_connection(
  stream: TcpStream,
  remote_addr: SocketAddr,
  app: Router,
  shutdown: CancellationToken,
) {
  let mut timed_stream = TimeoutStream::new(stream);
  timed_stream.set_read_timeout(Some(HTTP_READ_TIMEOUT));
  timed_stream.set_write_timeout(Some(HTTP_WRITE_TIMEOUT));
  let service =
    TowerToHyperService::new(app.layer(Extension(ConnectInfo(remote_addr))));
  let connection = Builder::new()
    .timer(TokioTimer::new())
    .header_read_timeout(HTTP_READ_TIMEOUT)
    .serve_connection(TokioIo::new(Box::pin(timed_stream)), service);
  tokio::pin!(connection);

  let result = tokio::select! {
    result = &mut connection => result,
    () = shutdown.cancelled() => {
      connection.as_mut().graceful_shutdown();
      connection.await
    }
  };

  if let Err(err) = result {
    debug!(error = %err, "HTTP connection failed");
  }
}

/// Spawns the task refreshing the unique visitor gauge on a fixed interval.
fn spawn_unique_gauge_updater(
  state: AppState,
  shutdown: CancellationToken,
) -> JoinHandle<()> {
  spawn(async move {
    let mut timer = interval(UNIQUES_UPDATE_PERIOD);
    loop {
      tokio::select! {
          _ = timer.tick() => state.metrics().update_unique_gauge(),
          () = shutdown.cancelled() => break,
      }
    }
  })
}

/// Waits for Ctrl-C or SIGTERM, then releases the shutdown token.
async fn shutdown_signal(shutdown: CancellationToken) {
  #[cfg(unix)]
  {
    match signal(SignalKind::terminate()) {
      Ok(mut terminate) => {
        tokio::select! {
            _ = ctrl_c() => {},
            _ = terminate.recv() => {},
        }
      },
      Err(err) => {
        warn!(error = %err, "could not install SIGTERM handler; waiting for Ctrl-C");
        let _result = ctrl_c().await;
      },
    }
  }

  #[cfg(not(unix))]
  {
    let _result = ctrl_c().await;
  }

  shutdown.cancel();
}
