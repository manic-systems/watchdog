use std::path::Path;

use parking_lot::Mutex;
use probabilistic_collections::hyperloglog::HyperLogLog;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::OffsetDateTime;

use crate::config::SaltRotation;

/// Errors returned while loading or saving unique visitor estimator state.
#[derive(Debug, Error)]
pub enum UniqueStateError {
  #[error("failed to read unique visitor state: {0}")]
  Read(#[source] std::io::Error),
  #[error("failed to write unique visitor state: {0}")]
  Write(#[source] std::io::Error),
  #[error("failed to serialize unique visitor state: {0}")]
  Serialize(#[source] postcard::Error),
  #[error("failed to deserialize unique visitor state: {0}")]
  Deserialize(#[source] postcard::Error),
}

/// Salted HyperLogLog estimator for privacy-preserving unique visitor counts.
pub struct UniquesEstimator {
  rotation: SaltRotation,
  inner:    Mutex<UniquesInner>,
}

#[derive(Serialize, Deserialize)]
struct PersistedUniques {
  salt_key: String,
  salt:     String,
  hll:      HyperLogLog<String>,
}

#[derive(Serialize)]
struct PersistedUniquesRef<'a> {
  salt_key: &'a str,
  salt:     &'a str,
  hll:      &'a HyperLogLog<String>,
}

struct UniquesInner {
  salt_key: String,
  salt:     String,
  hll:      HyperLogLog<String>,
}

impl UniquesEstimator {
  /// Creates a new estimator for the configured salt rotation period.
  pub fn new(rotation: SaltRotation) -> Self {
    let salt_key = salt_key(OffsetDateTime::now_utc(), rotation);
    Self {
      rotation,
      inner: Mutex::new(UniquesInner {
        salt: generate_salt(&salt_key),
        salt_key,
        hll: HyperLogLog::new(0.01),
      }),
    }
  }

  /// Adds one visitor observation using a salted hash of IP address and user
  /// agent.
  pub fn add(&self, ip: &str, user_agent: &str) {
    let mut inner = self.inner.lock();
    let current_key = salt_key(OffsetDateTime::now_utc(), self.rotation);
    if current_key != inner.salt_key {
      inner.salt_key = current_key;
      inner.salt = generate_salt(&inner.salt_key);
      inner.hll = HyperLogLog::new(0.01);
    }

    let visitor_hash = hash_visitor(ip, user_agent, &inner.salt);
    inner.hll.insert(&visitor_hash);
  }

  /// Returns the current estimated unique visitor count.
  pub fn estimate(&self) -> f64 {
    self.inner.lock().hll.len()
  }

  /// Loads persisted state when it belongs to the current rotation period.
  pub async fn load(&self, path: &Path) -> Result<(), UniqueStateError> {
    let data = match tokio::fs::read(path).await {
      Ok(data) => data,
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
      Err(err) => return Err(UniqueStateError::Read(err)),
    };

    let persisted: PersistedUniques =
      postcard::from_bytes(&data).map_err(UniqueStateError::Deserialize)?;
    let current_key = salt_key(OffsetDateTime::now_utc(), self.rotation);

    let mut inner = self.inner.lock();
    if persisted.salt_key == current_key {
      inner.salt_key = persisted.salt_key;
      inner.salt = persisted.salt;
      inner.hll = persisted.hll;
    } else {
      inner.salt_key = current_key;
      inner.salt = generate_salt(&inner.salt_key);
      inner.hll = HyperLogLog::new(0.01);
    }

    Ok(())
  }

  /// Saves the current estimator state to disk.
  pub async fn save(&self, path: &Path) -> Result<(), UniqueStateError> {
    let data = {
      let inner = self.inner.lock();
      let persisted = PersistedUniquesRef {
        salt_key: &inner.salt_key,
        salt:     &inner.salt,
        hll:      &inner.hll,
      };
      postcard::to_stdvec(&persisted).map_err(UniqueStateError::Serialize)?
    };

    if let Some(parent) = parent_dir(path) {
      tokio::fs::create_dir_all(parent)
        .await
        .map_err(UniqueStateError::Write)?;
    }

    tokio::fs::write(path, data)
      .await
      .map_err(UniqueStateError::Write)
  }

  /// Returns the active salt for tests that verify state restoration.
  #[cfg(test)]
  pub fn current_salt(&self) -> String {
    self.inner.lock().salt.clone()
  }
}

fn parent_dir(path: &Path) -> Option<&Path> {
  path
    .parent()
    .filter(|parent| !parent.as_os_str().is_empty())
}

fn salt_key(now: OffsetDateTime, rotation: SaltRotation) -> String {
  match rotation {
    SaltRotation::Daily => {
      format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
      )
    },
    SaltRotation::Hourly => {
      format!(
        "{:04}-{:02}-{:02}T{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour()
      )
    },
  }
}

fn generate_salt(key: &str) -> String {
  let digest = Sha256::digest(format!("watchdog-salt-{key}").as_bytes());
  hex::encode(digest)
}

fn hash_visitor(ip: &str, user_agent: &str, salt: &str) -> String {
  let digest = Sha256::digest(format!("{ip}|{user_agent}|{salt}").as_bytes());
  hex::encode(digest)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn estimates_unique_visitors() {
    let estimator = UniquesEstimator::new(SaltRotation::Daily);

    estimator.add("192.0.2.1", "ua-a");
    let first_estimate = estimator.estimate();
    estimator.add("192.0.2.1", "ua-a");
    assert_eq!(estimator.estimate(), first_estimate);

    estimator.add("192.0.2.2", "ua-b");

    assert!(estimator.estimate() > first_estimate);
  }

  #[test]
  fn skips_empty_parent_for_bare_state_filename() {
    assert_eq!(parent_dir(Path::new("hll.state")), None);
  }

  #[tokio::test]
  async fn persists_current_period_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hll.state");
    let estimator = UniquesEstimator::new(SaltRotation::Daily);

    estimator.add("192.0.2.1", "ua-a");
    estimator.save(&path).await.unwrap();

    let restored = UniquesEstimator::new(SaltRotation::Daily);
    restored.load(&path).await.unwrap();

    assert!(restored.estimate() >= 1.0);
    assert_eq!(restored.current_salt(), estimator.current_salt());
  }
}
