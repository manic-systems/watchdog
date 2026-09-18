use std::collections::BTreeMap;

use serde::Deserialize;
use thiserror::Error;
use url::Url;

use crate::limits::{
  MAX_ENGAGEMENT_SECONDS,
  MAX_EVENT_NAME_LEN,
  MAX_PATH_LEN,
  MAX_PROPERTY_KEY_LEN,
  MAX_PROPERTY_VALUE_LEN,
  MAX_REFERRER_LEN,
  MAX_WIDTH,
};

/// Validation errors for incoming analytics events.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EventError {
  #[error("domain required")]
  MissingDomain,
  #[error("domain not allowed")]
  DomainNotAllowed,
  #[error("path required")]
  MissingPath,
  #[error("event name too long")]
  EventNameTooLong,
  #[error("path too long")]
  PathTooLong,
  #[error("referrer too long")]
  ReferrerTooLong,
  #[error("invalid width")]
  InvalidWidth,
  #[error("invalid engagement time")]
  InvalidEngagement,
  #[error("invalid scroll depth")]
  InvalidScrollDepth,
}

/// Incoming browser analytics event payload.
///
/// Fields use compact JSON names to keep beacon payloads small while accepting
/// both Watchdog and Plausible-style shapes.
#[derive(Debug, Clone, Deserialize)]
pub struct Event {
  /// Beacon domain (`d`), lowercased during normalization.
  #[serde(default, rename = "d", alias = "domain")]
  domain:       String,
  /// Full page URL (`u`) used for domain and path fallback.
  #[serde(default, rename = "u", alias = "url")]
  url:          String,
  /// Path string or custom property map (`p`).
  #[serde(default, rename = "p", alias = "props")]
  payload:      PayloadField,
  /// Raw referrer string (`r`).
  #[serde(default, rename = "r", alias = "referrer")]
  referrer:     String,
  /// Explicit event name (`n`).
  #[serde(default, rename = "n", alias = "name")]
  name:         String,
  /// Engagement seconds or legacy event name (`e`).
  #[serde(default, rename = "e")]
  engagement:   EngagementOrLegacyEvent,
  /// Viewport width in CSS pixels (`w`).
  #[serde(default, rename = "w", alias = "screen_width")]
  width:        u16,
  /// Scroll depth percentage (`sd`).
  #[serde(default, rename = "sd")]
  scroll_depth: u8,
  /// Tab-local new session flag (`s`).
  #[serde(default, rename = "s")]
  new_session:  bool,
}

/// Beacon payload carried in the compact `p` field.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(untagged)]
enum PayloadField {
  /// Path string reported in `p`.
  Path(String),
  /// Custom property map reported in `p`.
  Properties(BTreeMap<String, EventProperty>),
  /// Explicit null payload, treated as missing.
  Null(()),
  /// Missing payload.
  #[default]
  Empty,
}

/// Supported custom property values in event payloads.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum EventProperty {
  /// String property value.
  String(String),
  /// Numeric property value.
  Number(f64),
  /// Boolean property value.
  Bool(bool),
  /// Null property value, filtered from labels.
  Null(()),
}
impl EventProperty {
  /// Converts the property value into a Prometheus label candidate.
  #[must_use]
  #[inline]
  pub fn label_value(&self) -> String {
    #[expect(
      clippy::pattern_type_mismatch,
      reason = "matching borrowed property keeps self borrowed, by-value \
                pattern would move it"
    )]
    match self {
      Self::String(value) => value.trim().to_owned(),
      Self::Number(value) if value.fract() == 0.0_f64 => {
        format!("{value:.0}")
      },
      Self::Number(value) => value.to_string(),
      Self::Bool(value) => value.to_string(),
      Self::Null(()) => String::new(),
    }
  }
}

/// Legacy engagement-or-name field from the beacon payload.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(untagged)]
enum EngagementOrLegacyEvent {
  /// Engagement seconds reported in `e`.
  Engagement(f64),
  /// Legacy event name reported in `e`.
  LegacyEvent(String),
  /// Missing engagement or legacy event.
  #[default]
  Empty,
}
impl Event {
  /// Returns the event domain, falling back to the host from `u` when `d` is
  /// absent.
  #[inline]
  #[must_use]
  pub fn normalize_domain(&self) -> String {
    let domain = self
      .domain
      .trim()
      .trim_end_matches('.')
      .to_ascii_lowercase();
    if !domain.is_empty() {
      return domain;
    }

    Url::parse(&self.url)
      .ok()
      .and_then(|url| url.host_str().map(str::to_owned))
      .unwrap_or_default()
      .trim()
      .trim_end_matches('.')
      .to_ascii_lowercase()
  }

  /// Returns the reported path, falling back to the parsed URL path.
  #[inline]
  #[must_use]
  pub fn path(&self) -> String {
    #[expect(
      clippy::pattern_type_mismatch,
      reason = "matching borrowed payload keeps self borrowed, by-value \
                pattern would move it"
    )]
    if let PayloadField::Path(path) = &self.payload {
      return path.trim().to_owned();
    }

    if let Ok(url) = Url::parse(&self.url) {
      let mut path = url.path().to_owned();
      if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
      }
      if let Some(fragment) = url.fragment() {
        path.push('#');
        path.push_str(fragment);
      }
      return path;
    }

    self.url.trim().to_owned()
  }

  /// Returns the raw referrer string from the payload.
  #[must_use]
  #[inline]
  pub fn referrer(&self) -> &str {
    &self.referrer
  }

  /// Returns the raw page URL string from the payload.
  #[must_use]
  #[inline]
  pub fn url(&self) -> &str {
    &self.url
  }

  /// Returns the normalized event name, including the legacy string `e` field.
  #[must_use]
  #[inline]
  #[expect(
    clippy::pattern_type_mismatch,
    reason = "matching on borrowed legacy field keeps ownership visible, \
              explicit & pattern would hide it"
  )]
  pub fn event_name(&self) -> String {
    let name = self.name.trim();
    if !name.is_empty() {
      return name.to_owned();
    }

    match &self.engagement {
      EngagementOrLegacyEvent::LegacyEvent(event) => event.trim().to_owned(),
      EngagementOrLegacyEvent::Engagement(_)
      | EngagementOrLegacyEvent::Empty => String::new(),
    }
  }

  /// Returns positive engagement duration reported in seconds.
  #[must_use]
  #[inline]
  #[expect(
    clippy::pattern_type_mismatch,
    reason = "matching on borrowed legacy field keeps ownership visible, \
              explicit & pattern would hide it"
  )]
  pub fn engagement_seconds(&self) -> Option<f64> {
    match &self.engagement {
      EngagementOrLegacyEvent::Engagement(seconds) if *seconds > 0.0_f64 => {
        Some(*seconds)
      },
      EngagementOrLegacyEvent::Engagement(_)
      | EngagementOrLegacyEvent::LegacyEvent(_)
      | EngagementOrLegacyEvent::Empty => None,
    }
  }

  /// Returns the reported viewport width in CSS pixels, or zero when absent.
  #[inline]
  #[must_use]
  pub const fn width(&self) -> u16 {
    self.width
  }

  /// Returns the reported scroll depth percentage.
  #[inline]
  #[must_use]
  pub const fn scroll_depth(&self) -> u8 {
    self.scroll_depth
  }

  /// Returns whether the browser reported a new tab-local session.
  #[inline]
  #[must_use]
  pub const fn is_new_session(&self) -> bool {
    self.new_session
  }

  /// Returns custom properties after trimming and applying per-label bounds.
  #[inline]
  #[must_use]
  #[expect(
    clippy::pattern_type_mismatch,
    reason = "matching on borrowed payload keeps ownership visible, explicit \
              & pattern would hide it"
  )]
  pub fn properties(&self) -> BTreeMap<String, String> {
    match &self.payload {
      PayloadField::Properties(properties) => {
        properties
          .iter()
          .map(|(raw_key, raw_value)| {
            let trimmed_key = raw_key.trim();
            let owned_value = raw_value.label_value();
            let trimmed_value = owned_value.trim();
            let key = if trimmed_key.len() > MAX_PROPERTY_KEY_LEN {
              "other".to_owned()
            } else {
              trimmed_key.to_owned()
            };
            let value = if trimmed_value.len() > MAX_PROPERTY_VALUE_LEN {
              "other".to_owned()
            } else {
              trimmed_value.to_owned()
            };
            (key, value)
          })
          .filter(|(key, value)| !key.is_empty() && !value.is_empty())
          .collect()
      },
      PayloadField::Path(_) | PayloadField::Null(()) | PayloadField::Empty => {
        BTreeMap::new()
      },
    }
  }

  /// Checks domain authorization and payload bounds before ingestion.
  ///
  /// # Errors
  ///
  /// Returns the first bound or authorization failure for the event.
  #[inline]
  pub fn validate<Allowed>(
    &self,
    domain_allowed: Allowed,
  ) -> Result<(), EventError>
  where
    Allowed: FnOnce(&str) -> bool,
  {
    let domain = self.normalize_domain();
    if domain.is_empty() {
      return Err(EventError::MissingDomain);
    }
    if !domain_allowed(&domain) {
      return Err(EventError::DomainNotAllowed);
    }

    self.validate_bounds()
  }

  fn validate_bounds(&self) -> Result<(), EventError> {
    let path = self.path();
    if path.is_empty() {
      return Err(EventError::MissingPath);
    }
    if path.len() > MAX_PATH_LEN {
      return Err(EventError::PathTooLong);
    }
    if self.event_name().len() > MAX_EVENT_NAME_LEN {
      return Err(EventError::EventNameTooLong);
    }
    if self.referrer.len() > MAX_REFERRER_LEN {
      return Err(EventError::ReferrerTooLong);
    }
    if self.width > MAX_WIDTH {
      return Err(EventError::InvalidWidth);
    }
    self.validate_telemetry()?;
    if self.scroll_depth > 100 {
      return Err(EventError::InvalidScrollDepth);
    }
    Ok(())
  }

  fn validate_telemetry(&self) -> Result<(), EventError> {
    #[expect(
      clippy::pattern_type_mismatch,
      reason = "matching borrowed engagement keeps self borrowed, by-value \
                pattern would move it"
    )]
    if let EngagementOrLegacyEvent::Engagement(seconds) = &self.engagement
      && (*seconds <= 0.0_f64
        || !seconds.is_finite()
        || *seconds > MAX_ENGAGEMENT_SECONDS)
    {
      return Err(EventError::InvalidEngagement);
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  #[expect(
    clippy::unwrap_used,
    reason = "test fixtures are static JSON, parse failure means the test is \
              wrong"
  )]
  fn validates_allowed_domain_and_limits() {
    let event: Event = serde_json::from_str(
      r#"{
                "d": "Example.COM.",
                "p": "/docs",
                "w": 1024
            }"#,
    )
    .unwrap();

    event.validate(|domain| domain == "example.com").unwrap();
    assert_eq!(event.path(), "/docs");
  }

  #[test]
  #[expect(
    clippy::unwrap_used,
    reason = "test fixtures are static JSON, parse failure means the test is \
              wrong"
  )]
  fn accepts_plausible_style_payload() {
    let event: Event = serde_json::from_str(
      r#"{
                "u": "https://example.com/docs?utm_source=newsletter",
                "n": "engagement",
                "p": {"tier": "paid", "amount": 42},
                "e": 12.5,
                "sd": 75,
                "s": true
            }"#,
    )
    .unwrap();

    event.validate(|domain| domain == "example.com").unwrap();
    assert_eq!(event.normalize_domain(), "example.com");
    assert_eq!(event.path(), "/docs?utm_source=newsletter");
    assert_eq!(event.event_name(), "engagement");
    assert_eq!(event.engagement_seconds(), Some(12.5_f64));
    assert_eq!(event.scroll_depth(), 75);
    assert!(event.is_new_session());
    assert_eq!(event.properties().get("amount"), Some(&"42".to_owned()));
  }

  #[test]
  #[expect(
    clippy::unwrap_used,
    reason = "test fixtures are static JSON, parse failure means the test is \
              wrong"
  )]
  fn collapses_overlong_property_labels() {
    let long_value = "x".repeat(MAX_PROPERTY_VALUE_LEN + 1);
    let event: Event = serde_json::from_str(&format!(
      r#"{{
                "d": "example.com",
                "u": "https://example.com/",
                "p": {{"url": "{long_value}"}}
            }}"#
    ))
    .unwrap();

    event.validate(|domain| domain == "example.com").unwrap();
    assert_eq!(event.properties().get("url"), Some(&"other".to_owned()));
  }

  #[test]
  #[expect(
    clippy::unwrap_used,
    reason = "test fixtures are static JSON, parse failure means the test is \
              wrong"
  )]
  fn ignores_null_property_values() {
    let event: Event = serde_json::from_str(
      r#"{
                "d": "example.com",
                "u": "https://example.com/",
                "p": {"empty": null, "plan": "pro"}
            }"#,
    )
    .unwrap();

    event.validate(|domain| domain == "example.com").unwrap();
    assert!(!event.properties().contains_key("empty"));
    assert_eq!(event.properties().get("plan"), Some(&"pro".to_owned()));
  }

  #[test]
  #[expect(
    clippy::unwrap_used,
    reason = "test fixtures are static JSON, parse failure means the test is \
              wrong"
  )]
  fn treats_null_property_payload_as_missing_payload() {
    let event: Event = serde_json::from_str(
      r#"{
                "d": "example.com",
                "u": "https://example.com/docs?utm_source=newsletter",
                "p": null
            }"#,
    )
    .unwrap();

    event.validate(|domain| domain == "example.com").unwrap();
    assert_eq!(event.path(), "/docs?utm_source=newsletter");
    assert!(event.properties().is_empty());
  }

  #[test]
  #[expect(
    clippy::unwrap_used,
    reason = "test fixtures are static JSON, parse failure means the test is \
              wrong"
  )]
  fn keeps_legacy_event_field_as_event_name() {
    let event: Event = serde_json::from_str(
      r#"{
                "d": "example.com",
                "p": "/",
                "e": "signup"
            }"#,
    )
    .unwrap();

    assert_eq!(event.event_name(), "signup");
    assert_eq!(event.engagement_seconds(), None);
  }

  #[test]
  #[expect(
    clippy::unwrap_used,
    reason = "test fixtures are static JSON, parse failure means the test is \
              wrong"
  )]
  fn rejects_unknown_domain() {
    let event: Event = serde_json::from_str(
      r#"{
                "d": "evil.example",
                "p": "/"
            }"#,
    )
    .unwrap();

    assert_eq!(
      event.validate(|domain| domain == "example.com"),
      Err(EventError::DomainNotAllowed)
    );
  }

  #[test]
  #[expect(
    clippy::unwrap_used,
    reason = "test fixtures are static JSON, parse failure means the test is \
              wrong"
  )]
  fn rejects_nonpositive_engagement() {
    for payload in [
      r#"{"d": "example.com", "p": "/", "e": -5}"#,
      r#"{"d": "example.com", "p": "/", "e": 0}"#,
    ] {
      let event: Event = serde_json::from_str(payload).unwrap();
      assert_eq!(
        event.validate(|domain| domain == "example.com"),
        Err(EventError::InvalidEngagement)
      );
    }
  }

  #[test]
  #[expect(
    clippy::unwrap_used,
    reason = "test fixtures are static JSON, parse failure means the test is \
              wrong"
  )]
  fn accepts_plausible_wire_format() {
    let event: Event = serde_json::from_str(
      r#"{
        "domain": "example.com",
        "name": "pageview",
        "url": "https://example.com/docs",
        "referrer": "https://news.example/",
        "screen_width": 1024,
        "props": {"tier": "paid"}
      }"#,
    )
    .unwrap();

    assert!(event.validate(|domain| domain == "example.com").is_ok());
    assert_eq!(event.path(), "/docs");
    assert_eq!(event.event_name(), "pageview");
    assert_eq!(event.properties().get("tier"), Some(&"paid".to_owned()));
  }
}
