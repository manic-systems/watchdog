use std::{collections::HashMap, sync::Arc, time::SystemTime};

use prometheus::{
    CounterVec, Encoder, Gauge, IntCounter, IntCounterVec, Opts, Registry, TextEncoder,
};

use crate::{
    BuildInfo,
    config::{CollectConfig, Config, ReferrerMode},
    uniques::UniquesEstimator,
};

#[derive(Debug, Clone)]
pub struct PageviewLabels {
    pub path: String,
    pub country: Option<String>,
    pub device: Option<String>,
    pub referrer: Option<String>,
    pub domain: Option<String>,
}

pub struct Metrics {
    registry: Registry,
    pageviews: CounterVec,
    custom_events: CounterVec,
    path_overflow: IntCounter,
    referrer_overflow: IntCounter,
    event_overflow: IntCounter,
    blocked_requests: IntCounterVec,
    daily_uniques: Gauge,
    pageview_label_names: Vec<&'static str>,
    collect: CollectConfig,
    uniques: Option<Arc<UniquesEstimator>>,
}

impl Metrics {
    pub fn new(config: &Config, build_info: &BuildInfo) -> prometheus::Result<Self> {
        let registry = Registry::new();
        let pageview_label_names = pageview_label_names(&config.site.collect);

        let pageviews = CounterVec::new(
            Opts::new("web_pageviews_total", "Total number of pageviews"),
            &pageview_label_names,
        )?;
        let custom_events = CounterVec::new(
            Opts::new("web_custom_events_total", "Total number of custom events"),
            &["event"],
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
            Box::new(custom_events.clone()),
            Box::new(path_overflow.clone()),
            Box::new(referrer_overflow.clone()),
            Box::new(event_overflow.clone()),
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
            custom_events,
            path_overflow,
            referrer_overflow,
            event_overflow,
            blocked_requests,
            daily_uniques,
            pageview_label_names,
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

    pub fn record_pageview(&self, labels: PageviewLabels) {
        let mut values = Vec::with_capacity(self.pageview_label_names.len());
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
        if self.collect.domain {
            values.push(sanitize_label(
                labels.domain.as_deref().unwrap_or("unknown"),
            ));
        }

        let refs = values.iter().map(String::as_str).collect::<Vec<_>>();
        self.pageviews.with_label_values(&refs).inc();
    }

    pub fn record_custom_event(&self, event_name: &str) {
        let event_name = sanitize_label(event_name);
        self.custom_events
            .with_label_values(&[event_name.as_str()])
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
}

fn pageview_label_names(collect: &CollectConfig) -> Vec<&'static str> {
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
    if collect.domain {
        labels.push("domain");
    }
    labels
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
        config.validate().unwrap();
        let metrics = Metrics::new(&config, &BuildInfo::current()).unwrap();

        metrics.record_pageview(PageviewLabels {
            path: "/".to_owned(),
            country: None,
            device: Some("desktop".to_owned()),
            referrer: Some("direct".to_owned()),
            domain: None,
        });

        let body = metrics.encode().unwrap();
        assert!(body.contains("web_pageviews_total"));
    }
}
