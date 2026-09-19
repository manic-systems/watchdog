use std::path::Path;

use cardinality_estimator_safe::{Element, Sketch};
use data_encoding::HEXLOWER;
use hmac_sha256::Hash as Sha256;
use jiff::{Timestamp, tz::Offset};
use parking_lot::Mutex;
use rand::Rng;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::SaltRotation;

type VisitorSketch = Sketch<14, 6>;

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
  hll:      VisitorSketch,
}

#[derive(Serialize)]
struct PersistedUniquesRef<'a> {
  salt_key: &'a str,
  salt:     &'a str,
  hll:      &'a VisitorSketch,
}

struct UniquesInner {
  salt_key: String,
  salt:     String,
  hll:      VisitorSketch,
}

impl UniquesInner {
  fn rotate_if_expired(&mut self, rotation: SaltRotation) {
    let current_key = salt_key(Timestamp::now(), rotation);
    if current_key == self.salt_key {
      return;
    }

    self.salt = generate_salt(&current_key);
    self.salt_key = current_key;
    self.hll = VisitorSketch::default();
  }
}

impl UniquesEstimator {
  /// Creates a new estimator for the configured salt rotation period.
  pub fn new(rotation: SaltRotation) -> Self {
    let salt_key = salt_key(Timestamp::now(), rotation);
    Self {
      rotation,
      inner: Mutex::new(UniquesInner {
        salt: generate_salt(&salt_key),
        salt_key,
        hll: VisitorSketch::default(),
      }),
    }
  }

  /// Adds one visitor observation using a salted hash of IP address and user
  /// agent.
  pub fn add(&self, ip: &str, user_agent: &str) {
    let mut inner = self.inner.lock();
    inner.rotate_if_expired(self.rotation);

    let visitor_hash = hash_visitor(ip, user_agent, &inner.salt);
    inner.hll.insert(Element::from_hashed(visitor_hash));
  }

  /// Returns the current estimated unique visitor count.
  pub fn estimate(&self) -> f64 {
    let mut inner = self.inner.lock();
    inner.rotate_if_expired(self.rotation);
    inner.hll.estimate() as f64
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

    let mut inner = self.inner.lock();
    *inner = UniquesInner {
      salt_key: persisted.salt_key,
      salt:     persisted.salt,
      hll:      persisted.hll,
    };

    inner.rotate_if_expired(self.rotation);

    Ok(())
  }

  /// Saves the current estimator state to disk.
  pub async fn save(&self, path: &Path) -> Result<(), UniqueStateError> {
    let data = {
      let mut inner = self.inner.lock();
      inner.rotate_if_expired(self.rotation);

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

    let temp_path = tmp_path(path);
    if let Err(err) = tokio::fs::write(&temp_path, &data).await {
      let _ = tokio::fs::remove_file(&temp_path).await;
      return Err(UniqueStateError::Write(err));
    }
    if let Err(err) = tokio::fs::rename(&temp_path, path).await {
      let _ = tokio::fs::remove_file(&temp_path).await;
      return Err(UniqueStateError::Write(err));
    }
    Ok(())
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

fn tmp_path(path: &Path) -> std::path::PathBuf {
  let mut name = path.as_os_str().to_owned();
  name.push(".tmp");
  std::path::PathBuf::from(name)
}

fn salt_key(now: Timestamp, rotation: SaltRotation) -> String {
  let datetime = Offset::UTC.to_datetime(now);

  match rotation {
    SaltRotation::Daily => {
      format!(
        "{:04}-{:02}-{:02}",
        datetime.year(),
        datetime.month(),
        datetime.day()
      )
    },
    SaltRotation::Hourly => {
      format!(
        "{:04}-{:02}-{:02}T{:02}",
        datetime.year(),
        datetime.month(),
        datetime.day(),
        datetime.hour()
      )
    },
  }
}

fn generate_salt(key: &str) -> String {
  let mut bytes = [0u8; 32];
  rand::rng().fill_bytes(&mut bytes);
  let mut hasher = Sha256::new();
  hasher.update(key.as_bytes());
  hasher.update(bytes);
  HEXLOWER.encode(&hasher.finalize())
}

fn hash_visitor(ip: &str, user_agent: &str, salt: &str) -> u64 {
  let digest = Sha256::hash(format!("{ip}|{user_agent}|{salt}").as_bytes());
  let bytes = digest[..8]
    .try_into()
    .expect("SHA-256 has at least eight bytes");
  u64::from_le_bytes(bytes)
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

  #[test]
  fn generates_unique_salts_per_period() {
    assert_ne!(generate_salt("2026-09-18"), generate_salt("2026-09-18"));
  }

  #[tokio::test]
  async fn saves_without_leaving_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hll.state");
    let estimator = UniquesEstimator::new(SaltRotation::Daily);

    estimator.save(&path).await.unwrap();

    assert!(path.exists());
    assert!(!tmp_path(&path).exists());
  }
}
