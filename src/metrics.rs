use std::{
  collections::{HashMap, HashSet},
  sync::Arc,
  time::SystemTime,
};

use parking_lot::Mutex;
#[cfg(target_os = "linux")]
use prometheus::process_collector::ProcessCollector;
use prometheus::{
  Encoder as _,
  Gauge,
  IntCounter,
  Opts,
  Registry,
  TextEncoder,
  core::{AtomicF64, AtomicU64, Collector},
};

use crate::{
  BuildInfo,
  config::{CollectConfig, Config, ReferrerMode, SaltRotation},
  counters::CounterFamily,
  limits::MAX_METRICS_RESPONSE_SIZE,
  uniques::UniquesEstimator,
};

/// Dimension labels shared by pageview, event, session, and engagement metrics.
#[derive(Debug, Clone, Default)]
pub struct DimensionLabels {
  pub path:            String,
  pub country:         Option<String>,
  pub device:          Option<String>,
  pub referrer:        Option<String>,
  pub referrer_source: Option<String>,
  pub utm_source:      Option<String>,
  pub utm_medium:      Option<String>,
  pub utm_campaign:    Option<String>,
  pub utm_content:     Option<String>,
  pub utm_term:        Option<String>,
  pub click_id:        Option<String>,
  pub browser:         Option<String>,
  pub os:              Option<String>,
  pub screen:          Option<String>,
  pub domain:          Option<String>,
}

/// Prometheus metric registry and bounded recording helpers.
pub struct Metrics {
  /// Registry exported by the metrics endpoint.
  registry:           Registry,
  /// Pageview counter keyed by dimension labels.
  pageviews:          CounterFamily<AtomicF64>,
  /// Non-pageview event counter keyed by event and dimensions.
  events:             CounterFamily<AtomicF64>,
  /// Aggregate custom-event counter keyed by event name.
  custom_events:      CounterFamily<AtomicF64>,
  /// Reported session-start counter keyed by dimensions.
  sessions:           CounterFamily<AtomicF64>,
  /// Active engagement time counter keyed by dimensions.
  engagement_seconds: CounterFamily<AtomicF64>,
  /// Scroll-depth counter keyed by dimensions and depth bucket.
  scroll_depth:       CounterFamily<AtomicU64>,
  /// Custom property observation counter keyed by event and pair.
  custom_properties:  CounterFamily<AtomicU64>,
  /// Shared budget bounding admitted series and bytes.
  series_budget:      Mutex<SeriesBudget>,
  /// Observations collapsed by the shared series budget.
  series_overflow:    IntCounter,
  /// Paths rejected by the path cardinality limit.
  path_overflow:      IntCounter,
  /// Referrers rejected by the referrer cardinality limit.
  referrer_overflow:  IntCounter,
  /// Custom events rejected by the event cardinality limit.
  event_overflow:     IntCounter,
  /// Dimension values collapsed by their cardinality limits.
  dimension_overflow: CounterFamily<AtomicU64>,
  /// Embedded asset requests blocked by security filters.
  blocked_requests:   CounterFamily<AtomicU64>,
  /// Estimated unique visitors for the current salt period.
  unique_visitors:    Gauge,
  /// Ordered label names matching dimension value order.
  label_names:        Vec<&'static str>,
  /// Collection flags selecting exported dimensions.
  collect:            CollectConfig,
  /// Unique visitor estimator when unique tracking is enabled.
  uniques:            Option<Arc<UniquesEstimator>>,
}

/// Budget tracking admitted series and estimated encoded bytes.
#[derive(Default)]
struct SeriesBudget {
  /// Admitted label sets keyed by metric name.
  series: HashMap<String, HashSet<Vec<String>>>,
  /// Estimated encoded bytes for admitted series.
  bytes:  usize,
}

impl Metrics {
  /// Creates a metrics registry configured from collection flags and build
  /// metadata.
  ///
  /// # Errors
  ///
  /// Returns `prometheus::Error` when a descriptor fails validation or a
  /// collector fails to register.
  #[inline]
  #[expect(
    clippy::too_many_lines,
    reason = "constructor wires sixteen fixed collectors, splitting would \
              hide registration order"
  )]
  pub fn new(
    config: &Config,
    build_info: &BuildInfo,
  ) -> prometheus::Result<Self> {
    let registry = Registry::new();
    let label_names = metric_label_names(&config.site.collect);

    let pageviews = CounterFamily::new(
      Opts::new("web_pageviews_total", "Total number of pageviews"),
      &label_names,
    )?;

    let mut event_label_names = vec!["event"];
    event_label_names.extend(label_names.iter().copied());
    let events = CounterFamily::new(
      Opts::new("web_events_total", "Total number of non-pageview events"),
      &event_label_names,
    )?;

    let custom_events = CounterFamily::new(
      Opts::new("web_custom_events_total", "Total number of custom events"),
      &["event"],
    )?;
    let sessions = CounterFamily::new(
      Opts::new("web_sessions_total", "Total number of reported sessions"),
      &label_names,
    )?;
    let engagement_seconds = CounterFamily::new(
      Opts::new(
        "web_engagement_seconds_total",
        "Total active engagement time reported by clients",
      ),
      &label_names,
    )?;

    let mut scroll_label_names = label_names.clone();
    scroll_label_names.push("depth");
    let scroll_depth = CounterFamily::new(
      Opts::new(
        "web_scroll_depth_total",
        "Total scroll-depth reports bucketed by percentage",
      ),
      &scroll_label_names,
    )?;

    let custom_properties = CounterFamily::new(
      Opts::new(
        "web_custom_properties_total",
        "Bounded custom property observations by event",
      ),
      &["event", "key", "value"],
    )?;

    let series_overflow = IntCounter::with_opts(Opts::new(
      "web_series_overflow_total",
      "Metric observations collapsed due to the shared series budget",
    ))?;

    let path_overflow = IntCounter::with_opts(Opts::new(
      "web_path_overflow_total",
      "Paths collapsed to other due to cardinality limit",
    ))?;
    let referrer_overflow = IntCounter::with_opts(Opts::new(
      "web_referrer_overflow_total",
      "Referrers rejected due to cardinality limit",
    ))?;
    let event_overflow = IntCounter::with_opts(Opts::new(
      "web_event_overflow_total",
      "Custom events rejected due to cardinality limit",
    ))?;
    let dimension_overflow = CounterFamily::new(
      Opts::new(
        "web_dimension_overflow_total",
        "Dimension values collapsed due to cardinality limit",
      ),
      &["dimension"],
    )?;
    let blocked_requests = CounterFamily::new(
      Opts::new(
        "web_blocked_requests_total",
        "Embedded web asset requests blocked by security filters",
      ),
      &["reason"],
    )?;
    let (unique_metric_name, unique_metric_help) =
      match config.site.salt_rotation {
        Some(SaltRotation::Hourly) => {
          (
            "web_hourly_unique_visitors",
            "Estimated unique visitors for the current hour",
          )
        },
        Some(SaltRotation::Daily) | None => {
          (
            "web_daily_unique_visitors",
            "Estimated unique visitors for the current day",
          )
        },
      };
    let unique_visitors =
      Gauge::with_opts(Opts::new(unique_metric_name, unique_metric_help))?;

    let build_info_metric = Gauge::with_opts(
      Opts::new(
        "watchdog_build_info",
        "Build metadata for the running watchdog instance",
      )
      .const_labels(HashMap::from([
        ("version".to_owned(), build_info.version.clone()),
        ("commit".to_owned(), build_info.commit.clone()),
        ("build_date".to_owned(), build_info.build_date.clone()),
      ])),
    )?;
    build_info_metric.set(1.0_f64);

    let start_time = Gauge::with_opts(Opts::new(
      "watchdog_start_time_seconds",
      "Unix timestamp of when the watchdog process started",
    ))?;
    start_time.set(
      SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0_f64, |duration| duration.as_secs_f64()),
    );

    let collectors: [Box<dyn Collector>; 16] = [
      Box::new(pageviews.clone()),
      Box::new(events.clone()),
      Box::new(custom_events.clone()),
      Box::new(sessions.clone()),
      Box::new(engagement_seconds.clone()),
      Box::new(scroll_depth.clone()),
      Box::new(custom_properties.clone()),
      Box::new(series_overflow.clone()),
      Box::new(path_overflow.clone()),
      Box::new(referrer_overflow.clone()),
      Box::new(event_overflow.clone()),
      Box::new(dimension_overflow.clone()),
      Box::new(blocked_requests.clone()),
      Box::new(unique_visitors.clone()),
      Box::new(build_info_metric),
      Box::new(start_time),
    ];
    for collector in collectors {
      registry.register(collector)?;
    }

    #[cfg(target_os = "linux")]
    registry.register(Box::new(ProcessCollector::for_self()))?;

    Ok(Self {
      registry,
      pageviews,
      events,
      custom_events,
      sessions,
      engagement_seconds,
      scroll_depth,
      custom_properties,
      series_budget: Mutex::new(SeriesBudget::default()),
      series_overflow,
      path_overflow,
      referrer_overflow,
      event_overflow,
      dimension_overflow,
      blocked_requests,
      unique_visitors,
      label_names,
      collect: config.site.collect.clone(),
      uniques: config
        .site
        .salt_rotation
        .map(|rotation| Arc::new(UniquesEstimator::new(rotation))),
    })
  }

  /// Returns the unique visitor estimator when unique tracking is enabled.
  #[inline]
  pub fn uniques(&self) -> Option<Arc<UniquesEstimator>> {
    self.uniques.clone()
  }

  /// Increments the pageview counter with configured dimension labels.
  #[inline]
  pub fn record_pageview(&self, labels: &DimensionLabels) {
    let values = self.label_values(labels);
    let bounded = self.bounded_labels(&self.pageviews, values);
    self.pageviews.with_label_values(bounded).inc();
  }

  /// Increments the non-pageview event counter with configured labels.
  #[inline]
  pub fn record_event(&self, event_name: &str, labels: &DimensionLabels) {
    let mut values = vec![sanitize_label(event_name)];
    values.extend(self.label_values(labels));
    let bounded = self.bounded_labels(&self.events, values);
    self.events.with_label_values(bounded).inc();
  }

  /// Increments the aggregate custom-event counter by event name.
  #[inline]
  pub fn record_custom_event(&self, event_name: &str) {
    let values = vec![sanitize_label(event_name)];
    let bounded = self.bounded_labels(&self.custom_events, values);
    self.custom_events.with_label_values(bounded).inc();
  }

  /// Increments the reported session-start counter.
  #[inline]
  pub fn record_session(&self, labels: &DimensionLabels) {
    let values = self.label_values(labels);
    let bounded = self.bounded_labels(&self.sessions, values);
    self.sessions.with_label_values(bounded).inc();
  }

  /// Adds positive finite engagement seconds to the engagement counter.
  #[inline]
  pub fn record_engagement_seconds(
    &self,
    labels: &DimensionLabels,
    seconds: f64,
  ) {
    if seconds <= 0.0_f64 || !seconds.is_finite() {
      return;
    }

    let values = self.label_values(labels);
    let bounded = self.bounded_labels(&self.engagement_seconds, values);
    self
      .engagement_seconds
      .with_label_values(bounded)
      .inc_by(seconds);
  }

  /// Increments the bucketed scroll-depth counter.
  #[inline]
  pub fn record_scroll_depth(&self, labels: &DimensionLabels, depth: u8) {
    let mut values = self.label_values(labels);
    values.push(scroll_depth_bucket(depth).to_owned());
    let bounded = self.bounded_labels(&self.scroll_depth, values);
    self.scroll_depth.with_label_values(bounded).inc();
  }

  /// Increments the custom-property observation counter.
  #[inline]
  pub fn record_custom_property(
    &self,
    event_name: &str,
    key: &str,
    value: &str,
  ) {
    let values = vec![
      sanitize_label(event_name),
      sanitize_label(key),
      sanitize_label(value),
    ];

    let bounded = self.bounded_labels(&self.custom_properties, values);
    self.custom_properties.with_label_values(bounded).inc();
  }

  /// Records that a path was collapsed because the path registry was full.
  #[inline]
  pub fn record_path_overflow(&self) {
    self.path_overflow.inc();
  }
  /// Records that a referrer was collapsed because the referrer registry was
  /// full.
  #[inline]
  pub fn record_referrer_overflow(&self) {
    self.referrer_overflow.inc();
  }

  /// Records that a custom event was collapsed because the event registry was
  /// full.
  #[inline]
  pub fn record_event_overflow(&self) {
    self.event_overflow.inc();
  }

  /// Records that a named dimension value was collapsed because its registry
  /// was full.
  #[inline]
  pub fn record_dimension_overflow(&self, dimension: &str) {
    let values = vec![sanitize_label(dimension)];
    let bounded = self.bounded_labels(&self.dimension_overflow, values);
    self.dimension_overflow.with_label_values(bounded).inc();
  }

  /// Records a blocked embedded-asset request by reason.
  #[inline]
  pub fn record_blocked_request(&self, reason: &'static str) {
    let values = vec![sanitize_label(reason)];
    let bounded = self.bounded_labels(&self.blocked_requests, values);
    self.blocked_requests.with_label_values(bounded).inc();
  }

  /// Adds a visitor observation to the unique visitor estimator, if enabled.
  #[inline]
  pub fn add_unique(&self, ip: &str, user_agent: &str) {
    if let Some(uniques) = self.uniques.as_ref() {
      uniques.add(ip, user_agent);
    }
  }

  /// Updates the exported unique visitor gauge from the estimator.
  #[inline]
  pub fn update_unique_gauge(&self) {
    if let Some(uniques) = self.uniques.as_ref() {
      self.unique_visitors.set(uniques.estimate());
    }
  }

  /// Encodes all registered metrics in Prometheus text format.
  ///
  /// # Errors
  ///
  /// Returns `prometheus::Error` when gathering or encoding the registry fails.
  #[inline]
  pub fn encode(&self) -> Result<String, prometheus::Error> {
    self.update_unique_gauge();

    let encoder = TextEncoder::new();
    let mut buffer = Vec::new();
    encoder.encode(&self.registry.gather(), &mut buffer)?;
    Ok(String::from_utf8(buffer).unwrap_or_default())
  }

  /// Bounds label cardinality against the shared series budget.
  #[expect(
    clippy::significant_drop_tightening,
    reason = "budget lock guards check, insert, and byte accounting as one \
              atomic admission"
  )]
  fn bounded_labels(
    &self,
    metric: &impl Collector,
    values: Vec<String>,
  ) -> Vec<String> {
    const METADATA_AND_OVERFLOW_RESERVE: usize = 64 * 1024;
    const MAX_ENCODED_F64_LEN: usize = 327;

    let descriptors = metric.desc();
    #[expect(
      clippy::expect_used,
      reason = "counter families are built with exactly one descriptor"
    )]
    let descriptor = descriptors
      .first()
      .expect("counter families have one descriptor");

    let mut budget = self.series_budget.lock();
    let state = &mut *budget;
    let entries = state.series.entry(descriptor.fq_name.clone()).or_default();

    if entries.contains(values.as_slice()) {
      return values;
    }

    let encoded_bytes = descriptor.fq_name.len()
      + MAX_ENCODED_F64_LEN
      + 3
      + descriptor
        .variable_labels
        .iter()
        .zip(&values)
        .map(|(name, value)| name.len() + 4 + 2 * value.len())
        .sum::<usize>();

    if state.bytes + encoded_bytes
      > MAX_METRICS_RESPONSE_SIZE - METADATA_AND_OVERFLOW_RESERVE
    {
      self.series_overflow.inc();
      return vec!["other".to_owned(); values.len()];
    }

    entries.insert(values.clone());
    state.bytes += encoded_bytes;
    values
  }

  /// Builds ordered label values matching the configured label names.
  fn label_values(&self, labels: &DimensionLabels) -> Vec<String> {
    let mut values = Vec::with_capacity(self.label_names.len());
    values.push(sanitize_label(&labels.path));

    if self.collect.country {
      values.push(sanitize_label(
        labels.country.as_deref().unwrap_or("unknown"),
      ));
    }
    if self.collect.device {
      values.push(sanitize_label(
        labels.device.as_deref().unwrap_or("unknown"),
      ));
    }
    if self.collect.referrer != ReferrerMode::Off {
      values.push(sanitize_label(
        labels.referrer.as_deref().unwrap_or("direct"),
      ));
    }
    if self.collect.acquisition {
      values.push(sanitize_label(
        labels.referrer_source.as_deref().unwrap_or("direct"),
      ));
      values.push(sanitize_label(
        labels.utm_source.as_deref().unwrap_or("none"),
      ));
      values.push(sanitize_label(
        labels.utm_medium.as_deref().unwrap_or("none"),
      ));
      values.push(sanitize_label(
        labels.utm_campaign.as_deref().unwrap_or("none"),
      ));
      values.push(sanitize_label(
        labels.utm_content.as_deref().unwrap_or("none"),
      ));
      values.push(sanitize_label(labels.utm_term.as_deref().unwrap_or("none")));
      values.push(sanitize_label(labels.click_id.as_deref().unwrap_or("none")));
    }
    if self.collect.browser {
      values.push(sanitize_label(
        labels.browser.as_deref().unwrap_or("unknown"),
      ));
    }
    if self.collect.os {
      values.push(sanitize_label(labels.os.as_deref().unwrap_or("unknown")));
    }
    if self.collect.screen {
      values.push(sanitize_label(
        labels.screen.as_deref().unwrap_or("unknown"),
      ));
    }
    if self.collect.domain {
      values.push(sanitize_label(
        labels.domain.as_deref().unwrap_or("unknown"),
      ));
    }

    values
  }
}

/// Builds ordered label names from the collection flags.
fn metric_label_names(collect: &CollectConfig) -> Vec<&'static str> {
  let mut labels = vec!["path"];
  if collect.country {
    labels.push("country");
  }
  if collect.device {
    labels.push("device");
  }
  if collect.referrer != ReferrerMode::Off {
    labels.push("referrer");
  }
  if collect.acquisition {
    labels.extend([
      "referrer_source",
      "utm_source",
      "utm_medium",
      "utm_campaign",
      "utm_content",
      "utm_term",
      "click_id",
    ]);
  }
  if collect.browser {
    labels.push("browser");
  }
  if collect.os {
    labels.push("os");
  }
  if collect.screen {
    labels.push("screen");
  }
  if collect.domain {
    labels.push("domain");
  }
  labels
}

/// Buckets scroll depth into labeled percentage bands.
const fn scroll_depth_bucket(depth: u8) -> &'static str {
  match depth {
    0..=24 => "0",
    25..=49 => "25",
    50..=74 => "50",
    75..=89 => "75",
    90..=99 => "90",
    _ => "100",
  }
}

/// Sanitizes a user-controlled value before it is used as a Prometheus label.
#[inline]
#[must_use]
pub fn sanitize_label(label: &str) -> String {
  const MAX_LABEL_VALUE_LEN: usize = 200;
  let trimmed = label.trim();

  if trimmed.len() > MAX_LABEL_VALUE_LEN || trimmed.is_empty() {
    return "other".to_owned();
  }

  let valid = trimmed.chars().all(|ch| !ch.is_control() && ch != '\u{7f}');

  if valid {
    trimmed.to_owned()
  } else {
    "other".to_owned()
  }
}

#[cfg(test)]
mod tests {
  use anyhow::Result;

  use super::*;
  use crate::config::Config;

  #[test]
  fn sanitizes_label_values() {
    assert_eq!(sanitize_label("/docs/:id"), "/docs/:id");
    assert_eq!(
      sanitize_label("Outbound Link: Click"),
      "Outbound Link: Click"
    );
    assert_eq!(sanitize_label("  newsletter  "), "newsletter");
    assert_eq!(sanitize_label(""), "other");
  }

  #[test]
  #[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions must panic to fail the test"
  )]
  fn records_metrics() -> Result<()> {
    let mut config = Config::default();
    config.site.domains = vec!["example.com".to_owned()];
    config.site.collect.acquisition = true;
    config.site.collect.browser = true;
    config.site.collect.os = true;
    config.site.collect.screen = true;
    config.validate()?;
    let metrics = Metrics::new(&config, &BuildInfo::current())?;

    let labels = DimensionLabels {
      path: "/".to_owned(),
      device: Some("desktop".to_owned()),
      referrer: Some("direct".to_owned()),
      referrer_source: Some("direct".to_owned()),
      utm_source: Some("newsletter".to_owned()),
      browser: Some("firefox".to_owned()),
      os: Some("linux".to_owned()),
      screen: Some("desktop".to_owned()),
      ..DimensionLabels::default()
    };

    metrics.record_pageview(&labels);
    metrics.record_event("signup", &labels);
    metrics.record_custom_event("signup");
    metrics.record_session(&labels);
    metrics.record_engagement_seconds(&labels, 3.5);
    metrics.record_scroll_depth(&labels, 75);
    metrics.record_custom_property("signup", "tier", "paid");

    let body = metrics.encode()?;
    assert!(body.contains("web_pageviews_total"));
    assert!(body.contains("web_events_total"));
    assert!(body.contains("web_sessions_total"));
    assert!(body.contains("web_engagement_seconds_total"));
    assert!(body.contains("web_scroll_depth_total"));
    assert!(body.contains("web_custom_properties_total"));
    assert!(body.contains("event=\"signup\""));
    assert!(body.contains("depth=\"75\""));
    assert!(body.contains("key=\"tier\",value=\"paid\""));
    Ok(())
  }

  #[test]
  #[expect(
    clippy::panic_in_result_fn,
    reason = "assertions define the metric naming contract"
  )]
  fn names_unique_metric_for_hourly_rotation() -> Result<()> {
    let mut config = Config::default();
    config.site.domains = vec!["example.com".to_owned()];
    config.site.salt_rotation = Some(SaltRotation::Hourly);
    config.validate()?;
    let metrics = Metrics::new(&config, &BuildInfo::current())?;

    let body = metrics.encode()?;
    assert!(body.contains("web_hourly_unique_visitors"));
    assert!(!body.contains("web_daily_unique_visitors"));
    Ok(())
  }
}
