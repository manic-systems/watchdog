use std::{collections::HashMap, sync::Arc, time::SystemTime};

use prometheus::{
  CounterVec, Encoder, Gauge, IntCounter, IntCounterVec, Opts, Registry,
  TextEncoder,
};

use crate::{
  BuildInfo,
  config::{CollectConfig, Config, ReferrerMode},
  uniques::UniquesEstimator,
};

#[derive(Debug, Clone, Default)]
pub struct DimensionLabels {
  pub path: String,
  pub country: Option<String>,
  pub device: Option<String>,
  pub referrer: Option<String>,
  pub referrer_source: Option<String>,
  pub utm_source: Option<String>,
  pub utm_medium: Option<String>,
  pub utm_campaign: Option<String>,
  pub utm_content: Option<String>,
  pub utm_term: Option<String>,
  pub click_id: Option<String>,
  pub browser: Option<String>,
  pub os: Option<String>,
  pub screen: Option<String>,
  pub domain: Option<String>,
}

pub struct Metrics {
  registry: Registry,
  pageviews: CounterVec,
  events: CounterVec,
  custom_events: CounterVec,
  sessions: CounterVec,
  engagement_seconds: CounterVec,
  scroll_depth: IntCounterVec,
  custom_properties: IntCounterVec,
  path_overflow: IntCounter,
  referrer_overflow: IntCounter,
  event_overflow: IntCounter,
  dimension_overflow: IntCounterVec,
  blocked_requests: IntCounterVec,
  daily_uniques: Gauge,
  label_names: Vec<&'static str>,
  collect: CollectConfig,
  uniques: Option<Arc<UniquesEstimator>>,
}

impl Metrics {
  pub fn new(
    config: &Config,
    build_info: &BuildInfo,
  ) -> prometheus::Result<Self> {
    let registry = Registry::new();
    let label_names = metric_label_names(&config.site.collect);

    let pageviews = CounterVec::new(
      Opts::new("web_pageviews_total", "Total number of pageviews"),
      &label_names,
    )?;

    let mut event_label_names = vec!["event"];
    event_label_names.extend(label_names.iter().copied());
    let events = CounterVec::new(
      Opts::new("web_events_total", "Total number of non-pageview events"),
      &event_label_names,
    )?;

    let custom_events = CounterVec::new(
      Opts::new("web_custom_events_total", "Total number of custom events"),
      &["event"],
    )?;
    let sessions = CounterVec::new(
      Opts::new("web_sessions_total", "Total number of reported sessions"),
      &label_names,
    )?;
    let engagement_seconds = CounterVec::new(
      Opts::new(
        "web_engagement_seconds_total",
        "Total active engagement time reported by clients",
      ),
      &label_names,
    )?;

    let mut scroll_label_names = label_names.clone();
    scroll_label_names.push("depth");
    let scroll_depth = IntCounterVec::new(
      Opts::new(
        "web_scroll_depth_total",
        "Total scroll-depth reports bucketed by percentage",
      ),
      &scroll_label_names,
    )?;

    let custom_properties = IntCounterVec::new(
      Opts::new(
        "web_custom_properties_total",
        "Bounded custom property observations by event",
      ),
      &["event", "key", "value"],
    )?;

    let path_overflow = IntCounter::with_opts(Opts::new(
      "web_path_overflow_total",
      "Paths rejected due to cardinality limit",
    ))?;
    let referrer_overflow = IntCounter::with_opts(Opts::new(
      "web_referrer_overflow_total",
      "Referrers rejected due to cardinality limit",
    ))?;
    let event_overflow = IntCounter::with_opts(Opts::new(
      "web_event_overflow_total",
      "Custom events rejected due to cardinality limit",
    ))?;
    let dimension_overflow = IntCounterVec::new(
      Opts::new(
        "web_dimension_overflow_total",
        "Dimension values collapsed due to cardinality limit",
      ),
      &["dimension"],
    )?;
    let blocked_requests = IntCounterVec::new(
      Opts::new(
        "web_blocked_requests_total",
        "Embedded web asset requests blocked by security filters",
      ),
      &["reason"],
    )?;
    let daily_uniques = Gauge::with_opts(Opts::new(
      "web_daily_unique_visitors",
      "Estimated unique visitors for the current salt period",
    ))?;

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
    build_info_metric.set(1.0);

    let start_time = Gauge::with_opts(Opts::new(
      "watchdog_start_time_seconds",
      "Unix timestamp of when the watchdog process started",
    ))?;
    start_time.set(
      SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64()),
    );

    for collector in [
      Box::new(pageviews.clone()) as Box<dyn prometheus::core::Collector>,
      Box::new(events.clone()),
      Box::new(custom_events.clone()),
      Box::new(sessions.clone()),
      Box::new(engagement_seconds.clone()),
      Box::new(scroll_depth.clone()),
      Box::new(custom_properties.clone()),
      Box::new(path_overflow.clone()),
      Box::new(referrer_overflow.clone()),
      Box::new(event_overflow.clone()),
      Box::new(dimension_overflow.clone()),
      Box::new(blocked_requests.clone()),
      Box::new(daily_uniques.clone()),
      Box::new(build_info_metric),
      Box::new(start_time),
    ] {
      registry.register(collector)?;
    }

    #[cfg(target_os = "linux")]
    registry.register(Box::new(
      prometheus::process_collector::ProcessCollector::for_self(),
    ))?;

    Ok(Self {
      registry,
      pageviews,
      events,
      custom_events,
      sessions,
      engagement_seconds,
      scroll_depth,
      custom_properties,
      path_overflow,
      referrer_overflow,
      event_overflow,
      dimension_overflow,
      blocked_requests,
      daily_uniques,
      label_names,
      collect: config.site.collect.clone(),
      uniques: config
        .site
        .salt_rotation
        .map(|rotation| Arc::new(UniquesEstimator::new(rotation))),
    })
  }

  pub fn uniques(&self) -> Option<Arc<UniquesEstimator>> {
    self.uniques.clone()
  }

  pub fn record_pageview(&self, labels: &DimensionLabels) {
    let values = self.label_values(labels);
    let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
    self.pageviews.with_label_values(&refs).inc();
  }

  pub fn record_event(&self, event_name: &str, labels: &DimensionLabels) {
    let mut values = vec![sanitize_label(event_name)];
    values.extend(self.label_values(labels));
    let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
    self.events.with_label_values(&refs).inc();
  }

  pub fn record_custom_event(&self, event_name: &str) {
    let event_name = sanitize_label(event_name);
    self
      .custom_events
      .with_label_values(&[event_name.as_str()])
      .inc();
  }

  pub fn record_session(&self, labels: &DimensionLabels) {
    let values = self.label_values(labels);
    let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
    self.sessions.with_label_values(&refs).inc();
  }

  pub fn record_engagement_seconds(
    &self,
    labels: &DimensionLabels,
    seconds: f64,
  ) {
    if seconds <= 0.0 || !seconds.is_finite() {
      return;
    }

    let values = self.label_values(labels);
    let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
    self
      .engagement_seconds
      .with_label_values(&refs)
      .inc_by(seconds);
  }

  pub fn record_scroll_depth(&self, labels: &DimensionLabels, depth: u8) {
    let mut values = self.label_values(labels);
    values.push(scroll_depth_bucket(depth).to_owned());
    let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
    self.scroll_depth.with_label_values(&refs).inc();
  }

  pub fn record_custom_property(
    &self,
    event_name: &str,
    key: &str,
    value: &str,
  ) {
    let event_name = sanitize_label(event_name);
    let key = sanitize_label(key);
    let value = sanitize_label(value);
    self
      .custom_properties
      .with_label_values(&[event_name.as_str(), key.as_str(), value.as_str()])
      .inc();
  }

  pub fn record_path_overflow(&self) {
    self.path_overflow.inc();
  }

  pub fn record_referrer_overflow(&self) {
    self.referrer_overflow.inc();
  }

  pub fn record_event_overflow(&self) {
    self.event_overflow.inc();
  }

  pub fn record_dimension_overflow(&self, dimension: &str) {
    self
      .dimension_overflow
      .with_label_values(&[dimension])
      .inc();
  }

  pub fn record_blocked_request(&self, reason: &'static str) {
    self.blocked_requests.with_label_values(&[reason]).inc();
  }

  pub fn add_unique(&self, ip: &str, user_agent: &str) {
    if let Some(uniques) = &self.uniques {
      uniques.add(ip, user_agent);
    }
  }

  pub fn update_unique_gauge(&self) {
    if let Some(uniques) = &self.uniques {
      self.daily_uniques.set(uniques.estimate());
    }
  }

  pub fn encode(&self) -> Result<String, prometheus::Error> {
    self.update_unique_gauge();

    let encoder = TextEncoder::new();
    let mut buffer = Vec::new();
    encoder.encode(&self.registry.gather(), &mut buffer)?;
    Ok(String::from_utf8(buffer).unwrap_or_default())
  }

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

fn scroll_depth_bucket(depth: u8) -> &'static str {
  match depth {
    0..=24 => "0",
    25..=49 => "25",
    50..=74 => "50",
    75..=89 => "75",
    90..=99 => "90",
    _ => "100",
  }
}

pub fn sanitize_label(label: &str) -> String {
  const MAX_LABEL_VALUE_LEN: usize = 200;

  if label.len() > MAX_LABEL_VALUE_LEN || label.is_empty() {
    return "other".to_owned();
  }

  let valid = label.chars().all(|ch| !ch.is_control() && ch != '\u{7f}');

  if valid {
    label.to_owned()
  } else {
    "other".to_owned()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::config::Config;

  #[test]
  fn sanitizes_label_values() {
    assert_eq!(sanitize_label("/docs/:id"), "/docs/:id");
    assert_eq!(
      sanitize_label("Outbound Link: Click"),
      "Outbound Link: Click"
    );
    assert_eq!(sanitize_label(""), "other");
  }

  #[test]
  fn records_metrics() {
    let mut config = Config::default();
    config.site.domains = vec!["example.com".to_owned()];
    config.site.collect.acquisition = true;
    config.site.collect.browser = true;
    config.site.collect.os = true;
    config.site.collect.screen = true;
    config.site.collect.properties = true;
    config.validate().unwrap();
    let metrics = Metrics::new(&config, &BuildInfo::current()).unwrap();

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

    let body = metrics.encode().unwrap();
    assert!(body.contains("web_pageviews_total"));
    assert!(body.contains("web_events_total"));
    assert!(body.contains("web_sessions_total"));
    assert!(body.contains("web_engagement_seconds_total"));
    assert!(body.contains("web_scroll_depth_total"));
    assert!(body.contains("web_custom_properties_total"));
  }
}
