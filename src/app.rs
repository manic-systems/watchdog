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
  Json,
  Router,
  body::Body,
  extract::{ConnectInfo, DefaultBodyLimit, Path as AxumPath, State},
  http::{HeaderMap, HeaderValue, Method, StatusCode, header},
  response::{IntoResponse, Response},
  routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ipnet::IpNet;
use rust_embed::RustEmbed;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tower_http::{
  cors::{Any, CorsLayer},
  trace::TraceLayer,
};
use url::{Url, form_urlencoded};
use uuid::Uuid;

use crate::{
  BuildInfo,
  config::{Config, CorsConfig, ReferrerMode},
  event::Event,
  limits::{MAX_EVENT_SIZE, MAX_METRICS_RESPONSE_SIZE},
  metrics::{DimensionLabels, Metrics, sanitize_label},
  normalize::{PathNormalizer, extract_referrer_domain, extract_referrer_url},
  ratelimit::IpRateLimiter,
  registry::BoundedRegistry,
};
#[derive(RustEmbed)]
#[folder = "web"]
struct WebAssets;

/// Errors returned while constructing application state or routes.
#[derive(Debug, Error)]
pub enum AppError {
  #[error("failed to initialize metrics: {0}")]
  Metrics(#[from] prometheus::Error),
  #[error("invalid trusted proxy {value}: {reason}")]
  TrustedProxy { value: String, reason: String },
  #[error("invalid CORS origin {value}: {source}")]
  CorsOrigin {
    value:  String,
    source: axum::http::header::InvalidHeaderValue,
  },
}

/// Shared application state used by HTTP handlers.
#[derive(Clone)]
pub struct AppState {
  inner: Arc<AppStateInner>,
}

struct AppStateInner {
  config:                  Config,
  allowed_domains:         HashSet<String>,
  allowed_events:          HashSet<String>,
  trusted_proxies:         Vec<IpNet>,
  ingestion_limiter:       Option<Arc<IpRateLimiter>>,
  metrics_limiter:         Option<Arc<IpRateLimiter>>,
  path_normalizer:         PathNormalizer,
  path_registry:           BoundedRegistry,
  referrer_registry:       BoundedRegistry,
  custom_event_registry:   BoundedRegistry,
  dimension_registry:      BoundedRegistry,
  property_key_registry:   BoundedRegistry,
  property_value_registry: BoundedRegistry,
  metrics:                 Arc<Metrics>,
}

impl AppState {
  /// Builds application state from validated configuration and build metadata.
  pub fn new(config: Config, build_info: BuildInfo) -> Result<Self, AppError> {
    let allowed_domains = config.site.domains.iter().cloned().collect();
    let allowed_events = config
      .site
      .custom_events
      .iter()
      .map(|event| sanitize_label(event))
      .collect();
    let trusted_proxies =
      parse_trusted_proxies(&config.security.trusted_proxies)?;
    let metrics = Arc::new(Metrics::new(&config, &build_info)?);

    Ok(Self {
      inner: Arc::new(AppStateInner {
        path_normalizer: PathNormalizer::new(config.site.path.clone()),
        path_registry: BoundedRegistry::new(config.limits.max_paths),
        referrer_registry: BoundedRegistry::new(config.limits.max_sources),
        custom_event_registry: BoundedRegistry::new(
          config.limits.max_custom_events,
        ),
        dimension_registry: BoundedRegistry::new(
          config.limits.max_dimension_values,
        ),
        property_key_registry: BoundedRegistry::new(
          config.limits.max_property_keys,
        ),
        property_value_registry: BoundedRegistry::new(
          config.limits.max_property_values,
        ),
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

  /// Returns the runtime configuration backing this state.
  pub fn config(&self) -> &Config {
    &self.inner.config
  }

  /// Returns the shared metrics registry.
  pub fn metrics(&self) -> Arc<Metrics> {
    self.inner.metrics.clone()
  }

  /// Returns the path used for persisted unique visitor state.
  pub fn state_path(&self) -> &Path {
    Path::new(&self.inner.config.server.state_path)
  }

  /// Restores persisted unique visitor state when unique tracking is enabled.
  pub async fn load_state(
    &self,
  ) -> Result<(), crate::uniques::UniqueStateError> {
    if let Some(uniques) = self.inner.metrics.uniques() {
      uniques.load(self.state_path()).await?;
    }
    Ok(())
  }

  /// Persists unique visitor state when unique tracking is enabled.
  pub async fn save_state(
    &self,
  ) -> Result<(), crate::uniques::UniqueStateError> {
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
    self
      .inner
      .trusted_proxies
      .iter()
      .any(|network| network.contains(&ip))
  }
}

/// Builds the Axum router for ingestion, metrics, health, and embedded assets.
pub fn router(state: AppState) -> Result<Router, AppError> {
  let ingestion_path = state.config().server.ingestion_path.clone();
  let metrics_path = state.config().server.metrics_path.clone();
  let cors = state.config().security.cors.clone();
  let mut ingestion_route =
    post(ingest).route_layer(DefaultBodyLimit::max(MAX_EVENT_SIZE));

  if cors.enabled {
    ingestion_route = ingestion_route.layer(cors_layer(&cors)?);
  }

  let metrics_route = get(metrics);

  Ok(
    Router::new()
      .route(&ingestion_path, ingestion_route)
      .route(&metrics_path, metrics_route)
      .route("/health", get(health))
      .route("/web/{*path}", get(static_asset))
      .layer(TraceLayer::new_for_http())
      .with_state(state),
  )
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
  let remote_ip = remote_addr.ip();
  let client_ip = state.client_ip(&headers, remote_ip);

  if let Some(limiter) = &state.inner.ingestion_limiter
    && !limiter.check(client_ip)
  {
    return response_with_request_id(StatusCode::TOO_MANY_REQUESTS, request_id);
  }

  if state.config().site.sampling < 1.0
    && rand::random::<f64>() >= state.config().site.sampling
  {
    return response_with_request_id(StatusCode::NO_CONTENT, request_id);
  }

  let event_domain = event.normalize_domain();
  if event
    .validate(|domain| state.inner.allowed_domains.contains(domain))
    .is_err()
  {
    return response_with_request_id(StatusCode::BAD_REQUEST, request_id);
  }

  let event_path = event.path();
  let normalized_path = admit_path(&state, &event_path);

  let user_agent = headers
    .get(header::USER_AGENT)
    .and_then(|value| value.to_str().ok())
    .unwrap_or_default();
  state
    .inner
    .metrics
    .add_unique(&client_ip.to_string(), user_agent);

  let labels = metric_labels(
    &state,
    &event,
    &event_path,
    &event_domain,
    normalized_path,
    user_agent,
  );
  let event_name = event.event_name();
  let is_pageview = is_pageview_event(&event_name);
  let event_label = (!is_pageview)
    .then(|| bounded_event_label(&state, &event_name))
    .flatten();

  if event.is_new_session() && state.config().site.collect.sessions {
    state.inner.metrics.record_session(&labels);
  }

  if is_pageview {
    record_pageview(&state, &labels);
  } else if let Some(event_label) = event_label.as_deref() {
    record_event(&state, event_label, &labels);
  }

  record_engagement(&state, &event, &labels);
  record_properties(
    &state,
    if is_pageview {
      Some("pageview")
    } else {
      event_label.as_deref()
    },
    &event,
  );

  response_with_request_id(StatusCode::NO_CONTENT, request_id)
}

fn admit_path(state: &AppState, event_path: &str) -> String {
  let normalized = state.inner.path_normalizer.normalize(event_path);
  if state.inner.path_registry.add(&normalized) {
    return normalized;
  }
  state.inner.metrics.record_path_overflow();
  "other".to_owned()
}

fn record_pageview(state: &AppState, labels: &DimensionLabels) {
  if !state.config().site.collect.pageviews {
    return;
  }

  state.inner.metrics.record_pageview(labels);
}

fn record_event(state: &AppState, event_name: &str, labels: &DimensionLabels) {
  state.inner.metrics.record_custom_event(event_name);
  state.inner.metrics.record_event(event_name, labels);
}

fn record_engagement(
  state: &AppState,
  event: &Event,
  labels: &DimensionLabels,
) {
  if !state.config().site.collect.engagement {
    return;
  }

  if let Some(seconds) = event.engagement_seconds() {
    state
      .inner
      .metrics
      .record_engagement_seconds(labels, seconds);
  }
  if event.scroll_depth() > 0 {
    state
      .inner
      .metrics
      .record_scroll_depth(labels, event.scroll_depth());
  }
}

fn record_properties(
  state: &AppState,
  event_name: Option<&str>,
  event: &Event,
) {
  if !state.config().site.collect.properties {
    return;
  }

  let Some(event_name) = event_name else {
    return;
  };

  for (key, value) in event.properties() {
    let key = sanitize_label(&key);
    let value = sanitize_label(&value);
    if !state.inner.property_key_registry.add(&key) {
      state
        .inner
        .metrics
        .record_dimension_overflow("property_key");
      continue;
    }

    let registry_key = format!("{key}={value}");
    let value = if state.inner.property_value_registry.add(&registry_key) {
      value
    } else {
      state
        .inner
        .metrics
        .record_dimension_overflow("property_value");
      "other".to_owned()
    };
    state
      .inner
      .metrics
      .record_custom_property(event_name, &key, &value);
  }
}

fn metric_labels(
  state: &AppState,
  event: &Event,
  event_path: &str,
  event_domain: &str,
  normalized_path: String,
  user_agent: &str,
) -> DimensionLabels {
  let collect = &state.config().site.collect;
  let acquisition_url = if event.url().trim().is_empty() {
    event_path
  } else {
    event.url()
  };
  let acquisition = collect.acquisition.then(|| {
    acquisition_labels(acquisition_url, event.referrer(), event_domain)
  });

  DimensionLabels {
    path:            normalized_path,
    country:         collect.country.then(|| "unknown".to_owned()),
    device:          collect.device.then(|| {
      classify_device(
        event.width(),
        user_agent,
        state.config().limits.device_breakpoints,
      )
    }),
    referrer:        referrer_label(state, event.referrer(), event_domain),
    referrer_source: acquisition.as_ref().map(|labels| {
      bounded_dimension(
        state,
        "referrer_source",
        labels.referrer_source.clone(),
        "direct",
      )
    }),
    utm_source:      acquisition.as_ref().map(|labels| {
      bounded_dimension(state, "utm_source", labels.utm_source.clone(), "none")
    }),
    utm_medium:      acquisition.as_ref().map(|labels| {
      bounded_dimension(state, "utm_medium", labels.utm_medium.clone(), "none")
    }),
    utm_campaign:    acquisition.as_ref().map(|labels| {
      bounded_dimension(
        state,
        "utm_campaign",
        labels.utm_campaign.clone(),
        "none",
      )
    }),
    utm_content:     acquisition.as_ref().map(|labels| {
      bounded_dimension(
        state,
        "utm_content",
        labels.utm_content.clone(),
        "none",
      )
    }),
    utm_term:        acquisition.as_ref().map(|labels| {
      bounded_dimension(state, "utm_term", labels.utm_term.clone(), "none")
    }),
    click_id:        acquisition.as_ref().map(|labels| {
      bounded_dimension(state, "click_id", labels.click_id.clone(), "none")
    }),
    browser:         collect.browser.then(|| {
      bounded_dimension(
        state,
        "browser",
        Some(classify_browser(user_agent)),
        "unknown",
      )
    }),
    os:              collect.os.then(|| {
      bounded_dimension(state, "os", Some(classify_os(user_agent)), "unknown")
    }),
    screen:          collect.screen.then(|| {
      bounded_dimension(
        state,
        "screen",
        Some(classify_screen(
          event.width(),
          state.config().limits.device_breakpoints,
        )),
        "unknown",
      )
    }),
    domain:          collect.domain.then(|| event_domain.to_owned()),
  }
}

fn referrer_label(
  state: &AppState,
  referrer: &str,
  event_domain: &str,
) -> Option<String> {
  let label = match state.config().site.collect.referrer {
    ReferrerMode::Off => return None,
    ReferrerMode::Domain => extract_referrer_domain(referrer, event_domain),
    ReferrerMode::Url => extract_referrer_url(referrer, event_domain),
  }
  .unwrap_or_else(|| "other".to_owned());
  let label = sanitize_label(&label);

  if matches!(label.as_str(), "direct" | "internal")
    || state.inner.referrer_registry.add(&label)
  {
    Some(label)
  } else {
    state.inner.metrics.record_referrer_overflow();
    Some("other".to_owned())
  }
}

fn bounded_dimension(
  state: &AppState,
  dimension: &'static str,
  value: Option<String>,
  fallback: &'static str,
) -> String {
  let value = value
    .and_then(|value| {
      let value = value.trim();
      (!value.is_empty()).then(|| sanitize_label(value))
    })
    .unwrap_or_else(|| fallback.to_owned());

  if matches!(
    value.as_str(),
    "direct" | "internal" | "none" | "unknown" | "other"
  ) {
    return value;
  }

  let registry_key = format!("{dimension}={value}");
  if state.inner.dimension_registry.add(&registry_key) {
    value
  } else {
    state.inner.metrics.record_dimension_overflow(dimension);
    "other".to_owned()
  }
}

#[derive(Default)]
struct AcquisitionLabels {
  referrer_source: Option<String>,
  utm_source:      Option<String>,
  utm_medium:      Option<String>,
  utm_campaign:    Option<String>,
  utm_content:     Option<String>,
  utm_term:        Option<String>,
  click_id:        Option<String>,
}

fn acquisition_labels(
  url: &str,
  referrer: &str,
  event_domain: &str,
) -> AcquisitionLabels {
  let mut labels = AcquisitionLabels {
    referrer_source: extract_referrer_domain(referrer, event_domain),
    ..AcquisitionLabels::default()
  };

  if let Some(query) = query_string(url) {
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
      let value = value.trim();
      if value.is_empty() {
        continue;
      }

      match key.to_ascii_lowercase().as_ref() {
        "utm_source" => set_once(&mut labels.utm_source, value),
        "utm_medium" => set_once(&mut labels.utm_medium, value),
        "utm_campaign" => set_once(&mut labels.utm_campaign, value),
        "utm_content" => set_once(&mut labels.utm_content, value),
        "utm_term" => set_once(&mut labels.utm_term, value),
        key if is_click_id_param(key) => set_once(&mut labels.click_id, key),
        _ => {},
      }
    }
  }

  labels
}

fn query_string(input: &str) -> Option<String> {
  if let Ok(url) = Url::parse(input) {
    return url.query().map(str::to_owned);
  }

  let path = input.split_once('#').map_or(input, |(path, _)| path);
  path.split_once('?').map(|(_, query)| query.to_owned())
}

fn set_once(target: &mut Option<String>, value: &str) {
  if target.is_none() {
    *target = Some(value.trim().to_owned());
  }
}

fn is_click_id_param(key: &str) -> bool {
  matches!(
    key,
    "gclid"
      | "gbraid"
      | "wbraid"
      | "fbclid"
      | "msclkid"
      | "ttclid"
      | "twclid"
      | "li_fat_id"
  )
}

fn is_pageview_event(event_name: &str) -> bool {
  let event_name = event_name.trim();
  event_name.is_empty() || event_name.eq_ignore_ascii_case("pageview")
}

fn bounded_event_label(state: &AppState, event_name: &str) -> Option<String> {
  let event_name = sanitize_label(event_name.trim());

  if !is_system_event(&event_name)
    && !state.inner.allowed_events.is_empty()
    && !state.inner.allowed_events.contains(&event_name)
  {
    return None;
  }

  if is_system_event(&event_name)
    || state.inner.custom_event_registry.add(&event_name)
  {
    Some(event_name)
  } else {
    state.inner.metrics.record_event_overflow();
    Some("other".to_owned())
  }
}

fn is_system_event(event_name: &str) -> bool {
  matches!(
    event_name,
    "engagement"
      | "Outbound Link: Click"
      | "Cloaked Link: Click"
      | "File Download"
      | "404"
      | "WP Form Completions"
      | "Form: Submission"
  )
}

async fn metrics(
  State(state): State<AppState>,
  ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
  headers: HeaderMap,
) -> Response {
  let remote_ip = remote_addr.ip();
  let client_ip = state.client_ip(&headers, remote_ip);
  if let Some(limiter) = &state.inner.metrics_limiter
    && !limiter.check(client_ip)
  {
    return StatusCode::TOO_MANY_REQUESTS.into_response();
  }

  if !metrics_authorized(&state, &headers) {
    return unauthorized_metrics_response();
  }

  match state.inner.metrics.encode() {
    Ok(body) if body.len() <= MAX_METRICS_RESPONSE_SIZE => {
      ([(header::CONTENT_TYPE, prometheus::TEXT_FORMAT)], body).into_response()
    },
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

  let expected = format!("{}:{}", auth.username, auth.password);
  let provided = format!("{username}:{password}");
  constant_time_equal(expected.as_bytes(), provided.as_bytes())
}

fn constant_time_equal(expected: &[u8], provided: &[u8]) -> bool {
  let mismatch = expected.len().ct_eq(&provided.len());
  let max_len = expected.len().max(provided.len());
  let mut diff = 0u8;
  for index in 0..max_len {
    let left = expected.get(index).copied().unwrap_or(0);
    let right = provided.get(index).copied().unwrap_or(0);
    diff |= left ^ right;
  }
  bool::from(mismatch & diff.ct_eq(&0))
}

fn unauthorized_metrics_response() -> Response {
  let mut response = StatusCode::UNAUTHORIZED.into_response();
  response.headers_mut().insert(
    header::WWW_AUTHENTICATE,
    HeaderValue::from_static("Basic realm=\"Metrics\""),
  );
  response
}

async fn static_asset(
  State(state): State<AppState>,
  AxumPath(path): AxumPath<String>,
) -> Response {
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

fn response_with_request_id(
  status: StatusCode,
  request_id: String,
) -> Response {
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

fn classify_screen(
  width: u16,
  breakpoints: crate::config::DeviceBreakpoints,
) -> String {
  if width == 0 {
    return "unknown".to_owned();
  }
  if width < breakpoints.mobile {
    return "mobile".to_owned();
  }
  if width < breakpoints.tablet {
    return "tablet".to_owned();
  }
  if width >= 1440 {
    return "wide".to_owned();
  }
  "desktop".to_owned()
}

fn classify_browser(user_agent: &str) -> String {
  let ua = user_agent.to_ascii_lowercase();
  if ua.is_empty() {
    return "unknown".to_owned();
  }
  if ua.contains("bot") || ua.contains("crawler") || ua.contains("spider") {
    return "bot".to_owned();
  }
  if ua.contains("edg/") || ua.contains("edge/") {
    return "edge".to_owned();
  }
  if ua.contains("opr/") || ua.contains("opera") {
    return "opera".to_owned();
  }
  if ua.contains("firefox/") || ua.contains("fxios/") {
    return "firefox".to_owned();
  }
  if ua.contains("chrome/") || ua.contains("crios/") || ua.contains("chromium/")
  {
    return "chrome".to_owned();
  }
  if ua.contains("safari/") {
    return "safari".to_owned();
  }
  "other".to_owned()
}

fn classify_os(user_agent: &str) -> String {
  let ua = user_agent.to_ascii_lowercase();
  if ua.is_empty() {
    return "unknown".to_owned();
  }
  if ua.contains("windows") {
    return "windows".to_owned();
  }
  if ua.contains("android") {
    return "android".to_owned();
  }
  if ua.contains("iphone") || ua.contains("ipad") || ua.contains("ipod") {
    return "ios".to_owned();
  }
  if ua.contains("mac os") || ua.contains("macintosh") {
    return "macos".to_owned();
  }
  if ua.contains("linux") || ua.contains("x11") {
    return "linux".to_owned();
  }
  "other".to_owned()
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
        HeaderValue::from_str(origin).map_err(|source| {
          AppError::CorsOrigin {
            value: origin.clone(),
            source,
          }
        })
      })
      .collect::<Result<Vec<_>, _>>()?;
    Ok(layer.allow_origin(origins))
  }
}

fn rate_limiter(limit: u32) -> Option<Arc<IpRateLimiter>> {
  NonZeroU32::new(limit).map(|limit| Arc::new(IpRateLimiter::per_minute(limit)))
}

fn parse_trusted_proxies(values: &[String]) -> Result<Vec<IpNet>, AppError> {
  values
    .iter()
    .map(|value| {
      if let Ok(network) = value.parse::<IpNet>() {
        return Ok(network);
      }

      let ip = IpAddr::from_str(value).map_err(|err| {
        AppError::TrustedProxy {
          value:  value.clone(),
          reason: err.to_string(),
        }
      })?;
      let prefix_len = if ip.is_ipv4() { 32 } else { 128 };
      IpNet::new(ip, prefix_len).map_err(|err| {
        AppError::TrustedProxy {
          value:  value.clone(),
          reason: err.to_string(),
        }
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
  use crate::{
    BuildInfo,
    config::{Config, DeviceBreakpoints},
  };

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
    assert_eq!(classify_screen(390, breakpoints), "mobile");
    assert_eq!(classify_screen(1440, breakpoints), "wide");
    assert_eq!(classify_browser("Mozilla/5.0 Firefox/120.0"), "firefox");
    assert_eq!(classify_os("Mozilla/5.0 (X11; Linux x86_64)"), "linux");
  }

  #[test]
  fn extracts_acquisition_labels() {
    let labels = acquisition_labels(
      "https://example.com/?utm_source=newsletter&utm_medium=email&gclid=abc",
      "https://news.ycombinator.com/item?id=1",
      "example.com",
    );

    assert_eq!(labels.utm_source, Some("newsletter".to_owned()));
    assert_eq!(labels.utm_medium, Some("email".to_owned()));
    assert_eq!(labels.click_id, Some("gclid".to_owned()));
    assert_eq!(labels.referrer_source, Some("ycombinator.com".to_owned()));

    let fragment_only =
      acquisition_labels("/docs#utm_source=fragment", "", "example.com");
    assert_eq!(fragment_only.utm_source, None);
  }

  #[test]
  fn bounded_dimension_uses_fallback_for_blank_values() {
    let mut config = Config::default();
    config.site.domains = vec!["example.com".to_owned()];
    config.validate().unwrap();
    let state = AppState::new(config, BuildInfo::current()).unwrap();

    assert_eq!(
      bounded_dimension(&state, "utm_source", Some("  ".to_owned()), "none"),
      "none"
    );
  }

  #[test]
  fn validates_embedded_asset_paths() {
    assert!(validate_asset_path("beacon.js").is_ok());
    assert_eq!(validate_asset_path("../secret.js"), Err("invalid_path"));
    assert_eq!(validate_asset_path(".env"), Err("dotfile"));
    assert_eq!(validate_asset_path("config.js"), Err("sensitive_file"));
    assert_eq!(validate_asset_path("image.png"), Err("invalid_extension"));
  }

  #[test]
  fn collapses_path_to_other_when_registry_full() {
    let mut config = Config::default();
    config.site.domains = vec!["example.com".to_owned()];
    config.limits.max_paths = 1;
    config.validate().unwrap();
    let state = AppState::new(config, BuildInfo::current()).unwrap();

    assert_eq!(admit_path(&state, "/first"), "/first");
    assert_eq!(admit_path(&state, "/second"), "other");
  }

  #[test]
  fn compares_metrics_credentials_in_constant_time() {
    assert!(constant_time_equal(b"admin:secret", b"admin:secret"));
    assert!(!constant_time_equal(b"admin:secret", b"admin:wrong"));
    assert!(!constant_time_equal(
      b"admin:secret",
      b"admin:longer-secret"
    ));
    assert!(!constant_time_equal(b"admin:secret", b"user:secret"));
  }
}
