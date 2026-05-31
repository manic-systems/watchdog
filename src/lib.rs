pub mod app;
pub mod config;
pub mod event;
pub mod limits;
pub mod metrics;
pub mod normalize;
pub mod registry;
pub mod server;
pub mod uniques;

#[derive(Debug, Clone)]
pub struct BuildInfo {
  pub version: String,
  pub commit: String,
  pub build_date: String,
}

impl BuildInfo {
  pub fn current() -> Self {
    Self {
      version: env!("CARGO_PKG_VERSION").to_owned(),
      commit: option_env!("WATCHDOG_COMMIT")
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
