use std::collections::BTreeMap;

use serde::Deserialize;
use thiserror::Error;
use url::Url;

use crate::limits::{
    MAX_ENGAGEMENT_SECONDS, MAX_EVENT_NAME_LEN, MAX_PATH_LEN, MAX_PROPERTY_KEY_LEN,
    MAX_PROPERTY_VALUE_LEN, MAX_REFERRER_LEN, MAX_WIDTH,
};

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

#[derive(Debug, Clone, Deserialize)]
pub struct Event {
    #[serde(default, rename = "d")]
    domain: String,
    #[serde(default, rename = "u")]
    url: String,
    #[serde(default, rename = "p")]
    payload: PayloadField,
    #[serde(default, rename = "r")]
    referrer: String,
    #[serde(default, rename = "n")]
    name: String,
    #[serde(default, rename = "e")]
    engagement_or_legacy_event: EngagementOrLegacyEvent,
    #[serde(default, rename = "w")]
    width: u16,
    #[serde(default, rename = "sd")]
    scroll_depth: u8,
    #[serde(default, rename = "s")]
    new_session: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(untagged)]
enum PayloadField {
    Path(String),
    Properties(BTreeMap<String, EventProperty>),
    #[default]
    Empty,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum EventProperty {
    String(String),
    Number(f64),
    Bool(bool),
}

impl EventProperty {
    pub fn label_value(&self) -> String {
        match self {
            Self::String(value) => value.trim().to_owned(),
            Self::Number(value) if value.fract() == 0.0 => format!("{value:.0}"),
            Self::Number(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(untagged)]
enum EngagementOrLegacyEvent {
    Engagement(f64),
    LegacyEvent(String),
    #[default]
    Empty,
}

impl Event {
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

    pub fn path(&self) -> String {
        if let PayloadField::Path(path) = &self.payload
            && !path.trim().is_empty()
        {
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

    pub fn referrer(&self) -> &str {
        &self.referrer
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn event_name(&self) -> String {
        let name = self.name.trim();
        if !name.is_empty() {
            return name.to_owned();
        }

        match &self.engagement_or_legacy_event {
            EngagementOrLegacyEvent::LegacyEvent(event) => event.trim().to_owned(),
            EngagementOrLegacyEvent::Engagement(_) | EngagementOrLegacyEvent::Empty => {
                String::new()
            }
        }
    }

    pub fn engagement_seconds(&self) -> Option<f64> {
        match self.engagement_or_legacy_event {
            EngagementOrLegacyEvent::Engagement(seconds) if seconds > 0.0 => Some(seconds),
            EngagementOrLegacyEvent::Engagement(_)
            | EngagementOrLegacyEvent::LegacyEvent(_)
            | EngagementOrLegacyEvent::Empty => None,
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn scroll_depth(&self) -> u8 {
        self.scroll_depth
    }

    pub fn is_new_session(&self) -> bool {
        self.new_session
    }

    pub fn properties(&self) -> BTreeMap<String, String> {
        match &self.payload {
            PayloadField::Properties(properties) => properties
                .iter()
                .map(|(key, value)| {
                    let key = key.trim();
                    let value = value.label_value();
                    let value = value.trim();
                    let key = if key.len() > MAX_PROPERTY_KEY_LEN {
                        "other".to_owned()
                    } else {
                        key.to_owned()
                    };
                    let value = if value.len() > MAX_PROPERTY_VALUE_LEN {
                        "other".to_owned()
                    } else {
                        value.to_owned()
                    };
                    (key, value)
                })
                .filter(|(key, value)| !key.is_empty() && !value.is_empty())
                .collect(),
            PayloadField::Path(_) | PayloadField::Empty => BTreeMap::new(),
        }
    }

    pub fn validate(&self, domain_allowed: impl FnOnce(&str) -> bool) -> Result<(), EventError> {
        let domain = self.normalize_domain();
        if domain.is_empty() {
            return Err(EventError::MissingDomain);
        }
        if !domain_allowed(&domain) {
            return Err(EventError::DomainNotAllowed);
        }

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
        if let EngagementOrLegacyEvent::Engagement(seconds) = self.engagement_or_legacy_event
            && (!seconds.is_finite() || seconds > MAX_ENGAGEMENT_SECONDS)
        {
            return Err(EventError::InvalidEngagement);
        }
        if self.scroll_depth > 100 {
            return Err(EventError::InvalidScrollDepth);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_allowed_domain_and_limits() {
        let event: Event = serde_json::from_str(
            r#"{
                "d": "Example.COM.",
                "p": "/docs",
                "w": 1024
            }"#,
        )
        .unwrap();

        assert!(event.validate(|domain| domain == "example.com").is_ok());
        assert_eq!(event.path(), "/docs");
    }

    #[test]
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

        assert!(event.validate(|domain| domain == "example.com").is_ok());
        assert_eq!(event.normalize_domain(), "example.com");
        assert_eq!(event.path(), "/docs?utm_source=newsletter");
        assert_eq!(event.event_name(), "engagement");
        assert_eq!(event.engagement_seconds(), Some(12.5));
        assert_eq!(event.scroll_depth(), 75);
        assert!(event.is_new_session());
        assert_eq!(event.properties().get("amount"), Some(&"42".to_owned()));
    }

    #[test]
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

        assert!(event.validate(|domain| domain == "example.com").is_ok());
        assert_eq!(event.properties().get("url"), Some(&"other".to_owned()));
    }

    #[test]
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
}
