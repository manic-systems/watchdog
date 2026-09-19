use std::{
  net::SocketAddr,
  path::{Path, PathBuf},
  str::FromStr,
};

use figment::{
  Figment,
  providers::{Env, Format},
};
use ipnet::IpNet;
use serde::{Deserialize, de::DeserializeOwned};
use thiserror::Error;
use toml::de::Error as TomlError;
/// Errors produced while loading or validating configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
  #[error("failed to read configuration: {0}")]
  Extract(#[source] Box<figment::Error>),
  #[error("configuration file not found: {0}")]
  ConfigFileMissing(PathBuf),
  #[error("server.listen_addr is not a valid socket address: {0}")]
  InvalidListenAddr(String),
  #[error("server.{field} contains unsupported characters")]
  InvalidServerPathChars { field: &'static str },
  #[error("limits.max_events_per_minute must be greater than 0")]
  InvalidMaxEventsPerMinute,
  #[error("limits.max_metrics_per_minute must be greater than 0")]
  InvalidMaxMetricsPerMinute,
  #[error("site.domains is required")]
  MissingDomains,
  #[error("site.sampling must be between 0.0 and 1.0")]
  InvalidSampling,
  #[error("limits.max_paths must be greater than 0")]
  InvalidMaxPaths,
  #[error("limits.max_sources must be greater than 0")]
  InvalidMaxSources,
  #[error("limits.max_custom_events must be greater than 0")]
  InvalidMaxCustomEvents,
  #[error("limits.max_dimension_values must be greater than 0")]
  InvalidMaxDimensionValues,
  #[error("limits.max_property_keys must be greater than 0")]
  InvalidMaxPropertyKeys,
  #[error("limits.max_property_values must be greater than 0")]
  InvalidMaxPropertyValues,
  #[error("limits.device_breakpoints must satisfy 0 < mobile < tablet")]
  InvalidDeviceBreakpoints,
  #[error(
    "security.metrics_auth: username and password are required when enabled"
  )]
  InvalidMetricsAuth,
  #[error("security.cors: allowed_origins is required when enabled")]
  InvalidCors,
  #[error("security.trusted_proxies contains an invalid IP or CIDR: {0}")]
  InvalidTrustedProxy(String),
  #[error("server.{field} must start with '/'")]
  InvalidServerPath { field: &'static str },
  #[error("server.metrics_path and server.ingestion_path must be distinct")]
  DuplicateServerPath,
  #[error("server.{field} conflicts with reserved route {path}")]
  ReservedServerPath { field: &'static str, path: String },
}

/// Complete runtime configuration for a Watchdog instance.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
  pub site:     SiteConfig,
  pub limits:   LimitsConfig,
  pub server:   ServerConfig,
  pub security: SecurityConfig,
}

/// Site-specific analytics behavior and collection controls.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SiteConfig {
  pub domains:       Vec<String>,
  pub salt_rotation: Option<SaltRotation>,
  pub sampling:      f64,
  pub collect:       CollectConfig,
  pub custom_events: Vec<String>,
  pub path:          PathConfig,
}

impl Default for SiteConfig {
  fn default() -> Self {
    Self {
      domains:       Vec::new(),
      salt_rotation: None,
      sampling:      1.0,
      collect:       CollectConfig::default(),
      custom_events: Vec::new(),
      path:          PathConfig::default(),
    }
  }
}

/// Salt rotation period used for unique visitor estimates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaltRotation {
  Daily,
  Hourly,
}

impl SaltRotation {
  /// Returns the TOML representation for this rotation period.
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Daily => "daily",
      Self::Hourly => "hourly",
    }
  }
}

/// Feature flags for dimensions and event types collected into metrics.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CollectConfig {
  pub pageviews:   bool,
  pub sessions:    bool,
  pub engagement:  bool,
  pub country:     bool,
  pub device:      bool,
  pub browser:     bool,
  pub os:          bool,
  pub screen:      bool,
  pub referrer:    ReferrerMode,
  pub acquisition: bool,
  pub properties:  bool,
  pub domain:      bool,
}

impl Default for CollectConfig {
  fn default() -> Self {
    Self {
      pageviews:   true,
      sessions:    true,
      engagement:  true,
      country:     false,
      device:      true,
      browser:     false,
      os:          false,
      screen:      false,
      referrer:    ReferrerMode::Domain,
      acquisition: false,
      properties:  false,
      domain:      false,
    }
  }
}

/// Referrer detail level retained in Prometheus labels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferrerMode {
  Off,
  #[default]
  Domain,
  Url,
}

/// Path normalization options applied before recording metrics.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathConfig {
  pub strip_query:               bool,
  pub strip_fragment:            bool,
  pub collapse_numeric_segments: bool,
  pub max_segments:              usize,
  pub normalize_trailing_slash:  bool,
}

impl Default for PathConfig {
  fn default() -> Self {
    Self {
      strip_query:               true,
      strip_fragment:            true,
      collapse_numeric_segments: true,
      max_segments:              5,
      normalize_trailing_slash:  true,
    }
  }
}

/// Cardinality and rate limits used to bound metrics growth.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
  pub max_paths:              usize,
  pub max_events_per_minute:  u32,
  pub max_sources:            usize,
  pub max_custom_events:      usize,
  pub max_dimension_values:   usize,
  pub max_property_keys:      usize,
  pub max_property_values:    usize,
  pub max_metrics_per_minute: u32,
  pub device_breakpoints:     DeviceBreakpoints,
}

impl Default for LimitsConfig {
  fn default() -> Self {
    Self {
      max_paths:              10_000,
      max_events_per_minute:  10_000,
      max_sources:            500,
      max_custom_events:      100,
      max_dimension_values:   1_000,
      max_property_keys:      50,
      max_property_values:    500,
      max_metrics_per_minute: 60,
      device_breakpoints:     DeviceBreakpoints::default(),
    }
  }
}

/// Screen-width breakpoints used for device and screen labels.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceBreakpoints {
  pub mobile: u16,
  pub tablet: u16,
}

impl Default for DeviceBreakpoints {
  fn default() -> Self {
    Self {
      mobile: 768,
      tablet: 1024,
    }
  }
}

/// Security controls for trusted proxies, CORS, and metrics access.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecurityConfig {
  pub trusted_proxies: Vec<String>,
  pub cors:            CorsConfig,
  pub metrics_auth:    AuthConfig,
}

/// CORS policy for the event ingestion endpoint.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct CorsConfig {
  pub enabled:         bool,
  pub allowed_origins: Vec<String>,
}

/// Optional HTTP Basic authentication for the metrics endpoint.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AuthConfig {
  pub enabled:  bool,
  pub username: String,
  pub password: String,
}

/// HTTP listener, endpoint, and state-file settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
  pub listen_addr:    String,
  pub metrics_path:   String,
  pub ingestion_path: String,
  pub state_path:     String,
}

impl Default for ServerConfig {
  fn default() -> Self {
    Self {
      listen_addr:    "127.0.0.1:8080".to_owned(),
      metrics_path:   "/metrics".to_owned(),
      ingestion_path: "/api/event".to_owned(),
      state_path:     "/var/lib/watchdog/hll.state".to_owned(),
    }
  }
}

/// Command-line overrides applied after file and environment loading.
#[derive(Debug, Default)]
pub struct Overrides {
  pub listen_addr:    Option<String>,
  pub metrics_path:   Option<String>,
  pub ingestion_path: Option<String>,
}

struct Toml;

impl Format for Toml {
  type Error = TomlError;

  const NAME: &'static str = "TOML";

  fn from_str<Value: DeserializeOwned>(
    input: &str,
  ) -> Result<Value, Self::Error> {
    toml::from_str(input)
  }
}

/// Loads configuration from defaults, an optional TOML file, environment, and
/// explicit command-line overrides.
pub fn load(
  path: Option<&Path>,
  overrides: Overrides,
) -> Result<Config, ConfigError> {
  let mut figment = Figment::new();

  if let Some(path) = path {
    if !path.exists() {
      return Err(ConfigError::ConfigFileMissing(path.to_path_buf()));
    }
    figment = figment.merge(Toml::file(path));
  } else if let Some(path) = default_config_path() {
    figment = figment.merge(Toml::file(path));
  }

  let mut config: Config = figment
    .merge(Env::prefixed("WATCHDOG_").split("__"))
    .extract()
    .map_err(|err| ConfigError::Extract(Box::new(err)))?;

  if let Some(listen_addr) = overrides.listen_addr {
    config.server.listen_addr = listen_addr;
  }
  if let Some(metrics_path) = overrides.metrics_path {
    config.server.metrics_path = metrics_path;
  }
  if let Some(ingestion_path) = overrides.ingestion_path {
    config.server.ingestion_path = ingestion_path;
  }

  config.validate()?;
  Ok(config)
}

fn default_config_path() -> Option<PathBuf> {
  ["config.toml", "/etc/watchdog/config.toml"]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.exists())
}

impl Config {
  /// Normalizes and validates the configuration in place.
  pub fn validate(&mut self) -> Result<(), ConfigError> {
    self.site.domains = self
      .site
      .domains
      .iter()
      .map(|domain| domain.trim().trim_end_matches('.').to_ascii_lowercase())
      .filter(|domain| !domain.is_empty())
      .collect();

    if self.site.domains.is_empty() {
      return Err(ConfigError::MissingDomains);
    }

    self.site.custom_events = self
      .site
      .custom_events
      .iter()
      .map(|event| event.trim().to_owned())
      .filter(|event| !event.is_empty())
      .collect();

    if !(0.0..=1.0).contains(&self.site.sampling) {
      return Err(ConfigError::InvalidSampling);
    }

    if self.limits.max_paths == 0 {
      return Err(ConfigError::InvalidMaxPaths);
    }

    if self.limits.max_sources == 0 {
      return Err(ConfigError::InvalidMaxSources);
    }

    if self.limits.max_custom_events == 0 {
      return Err(ConfigError::InvalidMaxCustomEvents);
    }

    if self.limits.max_dimension_values == 0 {
      return Err(ConfigError::InvalidMaxDimensionValues);
    }

    if self.limits.max_property_keys == 0 {
      return Err(ConfigError::InvalidMaxPropertyKeys);
    }

    if self.limits.max_property_values == 0 {
      return Err(ConfigError::InvalidMaxPropertyValues);
    }

    if self.limits.max_events_per_minute == 0 {
      return Err(ConfigError::InvalidMaxEventsPerMinute);
    }

    if self.limits.max_metrics_per_minute == 0 {
      return Err(ConfigError::InvalidMaxMetricsPerMinute);
    }

    if self.server.listen_addr.parse::<SocketAddr>().is_err() {
      return Err(ConfigError::InvalidListenAddr(
        self.server.listen_addr.clone(),
      ));
    }

    if self.limits.device_breakpoints.mobile == 0
      || self.limits.device_breakpoints.tablet == 0
      || self.limits.device_breakpoints.mobile
        >= self.limits.device_breakpoints.tablet
    {
      return Err(ConfigError::InvalidDeviceBreakpoints);
    }

    if self.security.metrics_auth.enabled
      && (self.security.metrics_auth.username.is_empty()
        || self.security.metrics_auth.password.is_empty())
    {
      return Err(ConfigError::InvalidMetricsAuth);
    }

    if self.security.cors.enabled
      && self.security.cors.allowed_origins.is_empty()
    {
      return Err(ConfigError::InvalidCors);
    }

    for proxy in &self.security.trusted_proxies {
      if IpNet::from_str(proxy).is_ok()
        || proxy.parse::<std::net::IpAddr>().is_ok()
      {
        continue;
      }
      return Err(ConfigError::InvalidTrustedProxy(proxy.clone()));
    }

    validate_endpoint_path("metrics_path", &self.server.metrics_path)?;
    validate_endpoint_path("ingestion_path", &self.server.ingestion_path)?;
    if self.server.metrics_path == self.server.ingestion_path {
      return Err(ConfigError::DuplicateServerPath);
    }
    validate_reserved_endpoint_path("metrics_path", &self.server.metrics_path)?;
    validate_reserved_endpoint_path(
      "ingestion_path",
      &self.server.ingestion_path,
    )?;

    Ok(())
  }
}

fn validate_endpoint_path(
  field: &'static str,
  path: &str,
) -> Result<(), ConfigError> {
  if !path.starts_with('/') {
    return Err(ConfigError::InvalidServerPath { field });
  }
  let supported = path
    .chars()
    .all(|char| char.is_ascii_alphanumeric() || "-._~/".contains(char));
  if !supported {
    return Err(ConfigError::InvalidServerPathChars { field });
  }
  Ok(())
}

fn validate_reserved_endpoint_path(
  field: &'static str,
  path: &str,
) -> Result<(), ConfigError> {
  if path == "/health" || path == "/web" || path.starts_with("/web/") {
    Err(ConfigError::ReservedServerPath {
      field,
      path: path.to_owned(),
    })
  } else {
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn applies_defaults_and_normalizes_domains() {
    let mut config = Config::default();
    config.site.domains = vec!["Example.COM.".to_owned()];
    config.validate().unwrap();

    assert_eq!(config.site.domains, ["example.com"]);
    assert_eq!(config.server.metrics_path, "/metrics");
    assert!(config.site.collect.sessions);
    assert!(config.site.collect.engagement);
    assert_eq!(config.limits.device_breakpoints.mobile, 768);
    assert_eq!(config.limits.max_dimension_values, 1_000);
  }

  #[test]
  fn rejects_invalid_configuration() {
    let mut config = Config::default();
    assert!(matches!(
      config.validate(),
      Err(ConfigError::MissingDomains)
    ));

    config.site.domains = vec!["example.com".to_owned()];
    config.limits.max_paths = 0;
    assert!(matches!(
      config.validate(),
      Err(ConfigError::InvalidMaxPaths)
    ));

    config.limits.max_paths = 1;
    config.limits.max_dimension_values = 0;
    assert!(matches!(
      config.validate(),
      Err(ConfigError::InvalidMaxDimensionValues)
    ));

    config.limits.max_dimension_values = 1;
    config.limits.device_breakpoints.mobile = 1024;
    config.limits.device_breakpoints.tablet = 768;
    assert!(matches!(
      config.validate(),
      Err(ConfigError::InvalidDeviceBreakpoints)
    ));

    config.limits.device_breakpoints = DeviceBreakpoints::default();
    config.server.ingestion_path = "/metrics".to_owned();
    assert!(matches!(
      config.validate(),
      Err(ConfigError::DuplicateServerPath)
    ));

    config.server.ingestion_path = "/web/beacon.js".to_owned();
    assert!(matches!(
      config.validate(),
      Err(ConfigError::ReservedServerPath { .. })
    ));
  }

  #[test]
  fn loads_toml_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
      &path,
      r#"
                [site]
                domains = ["example.com"]

                [server]
                listen_addr = "127.0.0.1:9090"
            "#,
    )
    .unwrap();

    let config = load(Some(&path), Overrides::default()).unwrap();
    assert_eq!(config.server.listen_addr, "127.0.0.1:9090");
    assert_eq!(config.limits.max_paths, 10_000);
  }

  #[test]
  fn rejects_zero_rate_limits_bad_listen_addr_and_route_chars() {
    let mut config = Config::default();
    config.site.domains = vec!["example.com".to_owned()];

    config.limits.max_events_per_minute = 0;
    assert!(matches!(
      config.validate(),
      Err(ConfigError::InvalidMaxEventsPerMinute)
    ));
    config.limits.max_events_per_minute = 1;

    config.limits.max_metrics_per_minute = 0;
    assert!(matches!(
      config.validate(),
      Err(ConfigError::InvalidMaxMetricsPerMinute)
    ));
    config.limits.max_metrics_per_minute = 1;

    config.server.listen_addr = "not-an-addr".to_owned();
    assert!(matches!(
      config.validate(),
      Err(ConfigError::InvalidListenAddr(_))
    ));
    config.server.listen_addr = "127.0.0.1:8080".to_owned();

    config.server.metrics_path = "/metrics/{id}".to_owned();
    assert!(matches!(
      config.validate(),
      Err(ConfigError::InvalidServerPathChars { .. })
    ));
  }

  #[test]
  fn rejects_missing_explicit_config_file() {
    let missing = Path::new("/nonexistent-watchdog-config.toml");
    assert!(matches!(
      load(Some(missing), Overrides::default()),
      Err(ConfigError::ConfigFileMissing(_))
    ));
  }
}
