use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read configuration: {0}")]
    Extract(#[source] Box<figment::Error>),
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
    #[error("security.metrics_auth: username and password are required when enabled")]
    InvalidMetricsAuth,
    #[error("security.cors: allowed_origins is required when enabled")]
    InvalidCors,
    #[error("security.trusted_proxies contains an invalid IP or CIDR: {0}")]
    InvalidTrustedProxy(String),
    #[error("server.{field} must start with '/'")]
    InvalidServerPath { field: &'static str },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub site: SiteConfig,
    pub limits: LimitsConfig,
    pub server: ServerConfig,
    pub security: SecurityConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SiteConfig {
    pub domains: Vec<String>,
    pub salt_rotation: Option<SaltRotation>,
    pub sampling: f64,
    pub collect: CollectConfig,
    pub custom_events: Vec<String>,
    pub path: PathConfig,
}

impl Default for SiteConfig {
    fn default() -> Self {
        Self {
            domains: Vec::new(),
            salt_rotation: Some(SaltRotation::Daily),
            sampling: 1.0,
            collect: CollectConfig::default(),
            custom_events: Vec::new(),
            path: PathConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaltRotation {
    Daily,
    Hourly,
}

impl SaltRotation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Hourly => "hourly",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CollectConfig {
    pub pageviews: bool,
    pub country: bool,
    pub device: bool,
    pub referrer: ReferrerMode,
    pub domain: bool,
}

impl Default for CollectConfig {
    fn default() -> Self {
        Self {
            pageviews: true,
            country: false,
            device: true,
            referrer: ReferrerMode::Domain,
            domain: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferrerMode {
    Off,
    #[default]
    Domain,
    Url,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathConfig {
    pub strip_query: bool,
    pub strip_fragment: bool,
    pub collapse_numeric_segments: bool,
    pub max_segments: usize,
    pub normalize_trailing_slash: bool,
}

impl Default for PathConfig {
    fn default() -> Self {
        Self {
            strip_query: true,
            strip_fragment: true,
            collapse_numeric_segments: true,
            max_segments: 5,
            normalize_trailing_slash: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
    pub max_paths: usize,
    pub max_events_per_minute: u32,
    pub max_sources: usize,
    pub max_custom_events: usize,
    pub max_metrics_per_minute: u32,
    pub device_breakpoints: DeviceBreakpoints,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_paths: 10_000,
            max_events_per_minute: 10_000,
            max_sources: 500,
            max_custom_events: 100,
            max_metrics_per_minute: 60,
            device_breakpoints: DeviceBreakpoints::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecurityConfig {
    pub trusted_proxies: Vec<String>,
    pub cors: CorsConfig,
    pub metrics_auth: AuthConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct CorsConfig {
    pub enabled: bool,
    pub allowed_origins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AuthConfig {
    pub enabled: bool,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub metrics_path: String,
    pub ingestion_path: String,
    pub state_path: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:8080".to_owned(),
            metrics_path: "/metrics".to_owned(),
            ingestion_path: "/api/event".to_owned(),
            state_path: "/var/lib/watchdog/hll.state".to_owned(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Overrides {
    pub listen_addr: Option<String>,
    pub metrics_path: Option<String>,
    pub ingestion_path: Option<String>,
}

pub fn load(path: Option<&Path>, overrides: Overrides) -> Result<Config, ConfigError> {
    let mut figment = Figment::from(Serialized::defaults(Config::default()));

    let config_path = path.map(Path::to_path_buf).or_else(default_config_path);

    if let Some(path) = config_path {
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

        if self.security.metrics_auth.enabled
            && (self.security.metrics_auth.username.is_empty()
                || self.security.metrics_auth.password.is_empty())
        {
            return Err(ConfigError::InvalidMetricsAuth);
        }

        if self.security.cors.enabled && self.security.cors.allowed_origins.is_empty() {
            return Err(ConfigError::InvalidCors);
        }

        for proxy in &self.security.trusted_proxies {
            if IpNet::from_str(proxy).is_ok() || proxy.parse::<std::net::IpAddr>().is_ok() {
                continue;
            }
            return Err(ConfigError::InvalidTrustedProxy(proxy.clone()));
        }

        validate_endpoint_path("metrics_path", &self.server.metrics_path)?;
        validate_endpoint_path("ingestion_path", &self.server.ingestion_path)?;

        Ok(())
    }
}

fn validate_endpoint_path(field: &'static str, path: &str) -> Result<(), ConfigError> {
    if path.starts_with('/') {
        Ok(())
    } else {
        Err(ConfigError::InvalidServerPath { field })
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
        assert_eq!(config.limits.device_breakpoints.mobile, 768);
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
}
