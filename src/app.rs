use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
    num::NonZeroU32,
    path::Path,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, DefaultBodyLimit, Path as AxumPath, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use ipnet::IpNet;
use rust_embed::RustEmbed;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use uuid::Uuid;

use crate::{
    BuildInfo,
    config::{Config, CorsConfig, ReferrerMode},
    event::Event,
    limits::{MAX_EVENT_SIZE, MAX_METRICS_RESPONSE_SIZE},
    metrics::{Metrics, PageviewLabels, sanitize_label},
    normalize::{PathNormalizer, extract_referrer_domain, extract_referrer_url},
    registry::BoundedRegistry,
};

#[derive(RustEmbed)]
#[folder = "web"]
struct WebAssets;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("failed to initialize metrics: {0}")]
    Metrics(#[from] prometheus::Error),
    #[error("invalid trusted proxy {value}: {reason}")]
    TrustedProxy { value: String, reason: String },
    #[error("invalid CORS origin {value}: {source}")]
    CorsOrigin {
        value: String,
        source: axum::http::header::InvalidHeaderValue,
    },
}

#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

struct AppStateInner {
    config: Config,
    allowed_domains: HashSet<String>,
    allowed_events: HashSet<String>,
    trusted_proxies: Vec<IpNet>,
    ingestion_limiter: Option<Arc<DefaultDirectRateLimiter>>,
    metrics_limiter: Option<Arc<DefaultDirectRateLimiter>>,
    path_normalizer: PathNormalizer,
    path_registry: BoundedRegistry,
    referrer_registry: BoundedRegistry,
    custom_event_registry: BoundedRegistry,
    metrics: Arc<Metrics>,
}

impl AppState {
    pub fn new(config: Config, build_info: BuildInfo) -> Result<Self, AppError> {
        let allowed_domains = config.site.domains.iter().cloned().collect();
        let allowed_events = config
            .site
            .custom_events
            .iter()
            .map(|event| sanitize_label(event))
            .collect();
        let trusted_proxies = parse_trusted_proxies(&config.security.trusted_proxies)?;
        let metrics = Arc::new(Metrics::new(&config, &build_info)?);

        Ok(Self {
            inner: Arc::new(AppStateInner {
                path_normalizer: PathNormalizer::new(config.site.path.clone()),
                path_registry: BoundedRegistry::new(config.limits.max_paths),
                referrer_registry: BoundedRegistry::new(config.limits.max_sources),
                custom_event_registry: BoundedRegistry::new(config.limits.max_custom_events),
                ingestion_limiter: rate_limiter(config.limits.max_events_per_minute),
                metrics_limiter: rate_limiter(config.limits.max_metrics_per_minute),
                allowed_domains,
                allowed_events,
                trusted_proxies,
                metrics,
                config,
            }),
        })
    }

    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    pub fn metrics(&self) -> Arc<Metrics> {
        self.inner.metrics.clone()
    }

    pub fn state_path(&self) -> &Path {
        Path::new(&self.inner.config.server.state_path)
    }

    pub async fn load_state(&self) -> Result<(), crate::uniques::UniqueStateError> {
        if let Some(uniques) = self.inner.metrics.uniques() {
            uniques.load(self.state_path()).await?;
        }
        Ok(())
    }

    pub async fn save_state(&self) -> Result<(), crate::uniques::UniqueStateError> {
        if let Some(uniques) = self.inner.metrics.uniques() {
            uniques.save(self.state_path()).await?;
        }
        Ok(())
    }

    fn client_ip(&self, headers: &HeaderMap, remote_ip: IpAddr) -> IpAddr {
        if self.inner.trusted_proxies.is_empty() || !self.is_trusted_ip(remote_ip) {
            return remote_ip;
        }

        if let Some(header) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
        {
            for ip in header.split(',').rev().map(str::trim) {
                if let Ok(ip) = ip.parse::<IpAddr>()
                    && !self.is_trusted_ip(ip)
                {
                    return ip;
                }
            }
        }

        if let Some(ip) = headers
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<IpAddr>().ok())
            && !self.is_trusted_ip(ip)
        {
            return ip;
        }

        remote_ip
    }

    fn is_trusted_ip(&self, ip: IpAddr) -> bool {
        self.inner
            .trusted_proxies
            .iter()
            .any(|network| network.contains(&ip))
    }
}

pub fn router(state: AppState) -> Result<Router, AppError> {
    let ingestion_path = state.config().server.ingestion_path.clone();
    let metrics_path = state.config().server.metrics_path.clone();
    let cors = state.config().security.cors.clone();
    let mut ingestion_route = post(ingest).route_layer(DefaultBodyLimit::max(MAX_EVENT_SIZE));

    if cors.enabled {
        ingestion_route = ingestion_route.layer(cors_layer(&cors)?);
    }

    let metrics_route = get(metrics);

    Ok(Router::new()
        .route(&ingestion_path, ingestion_route)
        .route(&metrics_path, metrics_route)
        .route("/health", get(health))
        .route("/web/{*path}", get(static_asset))
        .layer(TraceLayer::new_for_http())
        .with_state(state))
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

async fn ingest(
    State(state): State<AppState>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(event): Json<Event>,
) -> Response {
    let request_id = request_id(&headers);

    if let Some(limiter) = &state.inner.ingestion_limiter
        && limiter.check().is_err()
    {
        return response_with_request_id(StatusCode::TOO_MANY_REQUESTS, request_id);
    }

    if state.config().site.sampling < 1.0 && rand::random::<f64>() >= state.config().site.sampling {
        return response_with_request_id(StatusCode::NO_CONTENT, request_id);
    }

    let event_domain = event.normalize_domain();
    if event
        .validate(|domain| state.inner.allowed_domains.contains(domain))
        .is_err()
    {
        return response_with_request_id(StatusCode::BAD_REQUEST, request_id);
    }

    let normalized_path = state.inner.path_normalizer.normalize(&event.path);
    if !state.inner.path_registry.add(&normalized_path) {
        state.inner.metrics.record_path_overflow();
        return response_with_request_id(StatusCode::NO_CONTENT, request_id);
    }

    let remote_ip = remote_addr.ip();
    let client_ip = state.client_ip(&headers, remote_ip);
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    state
        .inner
        .metrics
        .add_unique(&client_ip.to_string(), user_agent);

    let event_name = sanitize_label(event.event.trim());
    if event.event.trim().is_empty() {
        record_pageview(&state, &event, &event_domain, normalized_path, user_agent);
    } else if state.inner.allowed_events.is_empty()
        || state.inner.allowed_events.contains(&event_name)
    {
        record_custom_event(&state, &event_name);
    }

    response_with_request_id(StatusCode::NO_CONTENT, request_id)
}

fn record_pageview(
    state: &AppState,
    event: &Event,
    event_domain: &str,
    normalized_path: String,
    user_agent: &str,
) {
    if !state.config().site.collect.pageviews {
        return;
    }

    let device = state.config().site.collect.device.then(|| {
        classify_device(
            event.width,
            user_agent,
            state.config().limits.device_breakpoints,
        )
    });
    let referrer = referrer_label(state, &event.referrer, event_domain);
    let domain = state
        .config()
        .site
        .collect
        .domain
        .then(|| event_domain.to_owned());
    let country = state
        .config()
        .site
        .collect
        .country
        .then(|| "unknown".to_owned());

    state.inner.metrics.record_pageview(PageviewLabels {
        path: normalized_path,
        country,
        device,
        referrer,
        domain,
    });
}

fn record_custom_event(state: &AppState, event_name: &str) {
    if state.inner.custom_event_registry.add(event_name) {
        state.inner.metrics.record_custom_event(event_name);
    } else {
        state.inner.metrics.record_event_overflow();
        state.inner.metrics.record_custom_event("other");
    }
}

fn referrer_label(state: &AppState, referrer: &str, event_domain: &str) -> Option<String> {
    let label = match state.config().site.collect.referrer {
        ReferrerMode::Off => return None,
        ReferrerMode::Domain => extract_referrer_domain(referrer, event_domain),
        ReferrerMode::Url => extract_referrer_url(referrer, event_domain),
    }
    .unwrap_or_else(|| "other".to_owned());

    if matches!(label.as_str(), "direct" | "internal") || state.inner.referrer_registry.add(&label)
    {
        Some(label)
    } else {
        state.inner.metrics.record_referrer_overflow();
        Some("other".to_owned())
    }
}

async fn metrics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !metrics_authorized(&state, &headers) {
        return unauthorized_metrics_response();
    }

    if let Some(limiter) = &state.inner.metrics_limiter
        && limiter.check().is_err()
    {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }

    match state.inner.metrics.encode() {
        Ok(body) if body.len() <= MAX_METRICS_RESPONSE_SIZE => {
            ([(header::CONTENT_TYPE, prometheus::TEXT_FORMAT)], body).into_response()
        }
        Ok(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn metrics_authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let auth = &state.config().security.metrics_auth;
    if !auth.enabled {
        return true;
    }

    let Some(encoded) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Basic "))
    else {
        return false;
    };

    let Ok(decoded) = BASE64.decode(encoded) else {
        return false;
    };
    let Ok(credentials) = String::from_utf8(decoded) else {
        return false;
    };
    let Some((username, password)) = credentials.split_once(':') else {
        return false;
    };

    username.as_bytes().ct_eq(auth.username.as_bytes()).into()
        && password.as_bytes().ct_eq(auth.password.as_bytes()).into()
}

fn unauthorized_metrics_response() -> Response {
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"Metrics\""),
    );
    response
}

async fn static_asset(State(state): State<AppState>, AxumPath(path): AxumPath<String>) -> Response {
    if let Err(reason) = validate_asset_path(&path) {
        state.inner.metrics.record_blocked_request(reason);
        return StatusCode::NOT_FOUND.into_response();
    }

    let Some(asset) = WebAssets::get(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let content_type = mime_guess::from_path(&path).first_or_octet_stream();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type.as_ref())
        .body(Body::from(asset.data.into_owned()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn request_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().simple().to_string())
}

fn response_with_request_id(status: StatusCode, request_id: String) -> Response {
    let mut response = status.into_response();
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

fn classify_device(
    width: u16,
    user_agent: &str,
    breakpoints: crate::config::DeviceBreakpoints,
) -> String {
    let ua = user_agent.to_ascii_lowercase();

    if ua.contains("tablet")
        || ua.contains("ipad")
        || (ua.contains("android") && !ua.contains("mobile"))
    {
        return "tablet".to_owned();
    }

    if ua.contains("mobile")
        || ua.contains("iphone")
        || ua.contains("ipod")
        || ua.contains("windows phone")
        || ua.contains("blackberry")
    {
        return "mobile".to_owned();
    }

    if width > 0 {
        if width < breakpoints.mobile {
            return "mobile".to_owned();
        }
        if width < breakpoints.tablet {
            return "tablet".to_owned();
        }
        return "desktop".to_owned();
    }

    if user_agent.is_empty() {
        "unknown".to_owned()
    } else {
        "desktop".to_owned()
    }
}

fn cors_layer(config: &CorsConfig) -> Result<CorsLayer, AppError> {
    let layer = CorsLayer::new()
        .allow_methods([Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE])
        .max_age(Duration::from_secs(86_400));

    if config.allowed_origins.iter().any(|origin| origin == "*") {
        Ok(layer.allow_origin(Any))
    } else {
        let origins = config
            .allowed_origins
            .iter()
            .map(|origin| {
                HeaderValue::from_str(origin).map_err(|source| AppError::CorsOrigin {
                    value: origin.clone(),
                    source,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(layer.allow_origin(origins))
    }
}

fn rate_limiter(limit: u32) -> Option<Arc<DefaultDirectRateLimiter>> {
    NonZeroU32::new(limit).map(|limit| Arc::new(RateLimiter::direct(Quota::per_minute(limit))))
}

fn parse_trusted_proxies(values: &[String]) -> Result<Vec<IpNet>, AppError> {
    values
        .iter()
        .map(|value| {
            if let Ok(network) = value.parse::<IpNet>() {
                return Ok(network);
            }

            let ip = IpAddr::from_str(value).map_err(|err| AppError::TrustedProxy {
                value: value.clone(),
                reason: err.to_string(),
            })?;
            let prefix_len = if ip.is_ipv4() { 32 } else { 128 };
            IpNet::new(ip, prefix_len).map_err(|err| AppError::TrustedProxy {
                value: value.clone(),
                reason: err.to_string(),
            })
        })
        .collect()
}

fn validate_asset_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() || path.ends_with('/') {
        return Err("directory_listing");
    }

    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err("invalid_path");
        }

        if segment.starts_with('.') {
            return Err("dotfile");
        }

        let lower = segment.to_ascii_lowercase();
        if lower.contains(".env")
            || lower.contains("config")
            || lower.ends_with(".bak")
            || lower.ends_with('~')
        {
            return Err("sensitive_file");
        }
    }

    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some("js" | "html" | "css") => Ok(()),
        _ => Err("invalid_extension"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DeviceBreakpoints;

    #[test]
    fn classifies_devices() {
        let breakpoints = DeviceBreakpoints::default();

        assert_eq!(
            classify_device(0, "Mozilla/5.0 (iPhone)", breakpoints),
            "mobile"
        );
        assert_eq!(classify_device(900, "", breakpoints), "tablet");
        assert_eq!(classify_device(1440, "", breakpoints), "desktop");
        assert_eq!(classify_device(0, "", breakpoints), "unknown");
    }

    #[test]
    fn validates_embedded_asset_paths() {
        assert!(validate_asset_path("beacon.js").is_ok());
        assert_eq!(validate_asset_path("../secret.js"), Err("invalid_path"));
        assert_eq!(validate_asset_path(".env"), Err("dotfile"));
        assert_eq!(validate_asset_path("config.js"), Err("sensitive_file"));
        assert_eq!(validate_asset_path("image.png"), Err("invalid_extension"));
    }
}
