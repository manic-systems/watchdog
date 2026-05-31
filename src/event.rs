use serde::Deserialize;
use thiserror::Error;

use crate::limits::{MAX_PATH_LEN, MAX_REFERRER_LEN, MAX_WIDTH};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EventError {
    #[error("domain required")]
    MissingDomain,
    #[error("domain not allowed")]
    DomainNotAllowed,
    #[error("path required")]
    MissingPath,
    #[error("path too long")]
    PathTooLong,
    #[error("referrer too long")]
    ReferrerTooLong,
    #[error("invalid width")]
    InvalidWidth,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Event {
    #[serde(rename = "d")]
    pub domain: String,
    #[serde(rename = "p")]
    pub path: String,
    #[serde(default, rename = "r")]
    pub referrer: String,
    #[serde(default, rename = "e")]
    pub event: String,
    #[serde(default, rename = "w")]
    pub width: u16,
}

impl Event {
    pub fn normalize_domain(&self) -> String {
        self.domain
            .trim()
            .trim_end_matches('.')
            .to_ascii_lowercase()
    }

    pub fn validate(&self, domain_allowed: impl FnOnce(&str) -> bool) -> Result<(), EventError> {
        let domain = self.normalize_domain();
        if domain.is_empty() {
            return Err(EventError::MissingDomain);
        }
        if !domain_allowed(&domain) {
            return Err(EventError::DomainNotAllowed);
        }
        if self.path.is_empty() {
            return Err(EventError::MissingPath);
        }
        if self.path.len() > MAX_PATH_LEN {
            return Err(EventError::PathTooLong);
        }
        if self.referrer.len() > MAX_REFERRER_LEN {
            return Err(EventError::ReferrerTooLong);
        }
        if self.width > MAX_WIDTH {
            return Err(EventError::InvalidWidth);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_allowed_domain_and_limits() {
        let event = Event {
            domain: "Example.COM.".to_owned(),
            path: "/docs".to_owned(),
            referrer: String::new(),
            event: String::new(),
            width: 1024,
        };

        assert!(event.validate(|domain| domain == "example.com").is_ok());
    }

    #[test]
    fn rejects_unknown_domain() {
        let event = Event {
            domain: "evil.example".to_owned(),
            path: "/".to_owned(),
            referrer: String::new(),
            event: String::new(),
            width: 0,
        };

        assert_eq!(
            event.validate(|domain| domain == "example.com"),
            Err(EventError::DomainNotAllowed)
        );
    }
}
