//! Library entry points for Watchdog's HTTP server, configuration, and
//! aggregate metrics pipeline.

pub mod app;
pub mod config;
/// Shared counter family used by the metrics pipeline.
mod counters;
pub mod event;
pub mod limits;
pub mod metrics;
pub mod normalize;
pub mod ratelimit;
pub mod registry;
pub mod server;
pub mod uniques;
/// Build metadata exposed through metrics and logs.
#[derive(Debug, Clone)]
pub struct BuildInfo {
  /// Package version compiled into the binary.
  pub version:    String,
  /// Source revision supplied by the build environment, if available.
  pub commit:     String,
  /// Build timestamp supplied by the build environment, if available.
  pub build_date: String,
}

impl BuildInfo {
  /// Returns build metadata embedded at compile time.
  #[inline]
  #[must_use]
  pub fn current() -> Self {
    Self {
      version:    env!("CARGO_PKG_VERSION").to_owned(),
      commit:     option_env!("WATCHDOG_COMMIT")
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_owned(),
      build_date: option_env!("WATCHDOG_BUILD_DATE")
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_owned(),
    }
  }
}
