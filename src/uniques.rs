use std::{
  io::{Error as IoError, ErrorKind},
  path::{Path, PathBuf},
};

use cardinality_estimator_safe::{Element, Sketch};
use data_encoding::HEXLOWER;
use hmac_sha256::Hash as Sha256;
use jiff::{Timestamp, tz::Offset};
use parking_lot::Mutex;
use rand::Rng as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs;

use crate::config::SaltRotation;

/// `HyperLogLog` sketch storing salted visitor hashes.
type VisitorSketch = Sketch<14, 6>;

/// Errors returned while loading or saving unique visitor estimator state.
#[derive(Debug, Error)]
pub enum UniqueStateError {
  #[error("failed to read unique visitor state: {0}")]
  Read(#[source] IoError),
  #[error("failed to write unique visitor state: {0}")]
  Write(#[source] IoError),
  #[error("failed to serialize unique visitor state: {0}")]
  Serialize(#[source] postcard::Error),
  #[error("failed to deserialize unique visitor state: {0}")]
  Deserialize(#[source] postcard::Error),
}

/// Salted `HyperLogLog` estimator for privacy-preserving unique visitor counts.
pub struct UniquesEstimator {
  /// Salt rotation period controlling estimator resets.
  rotation: SaltRotation,
  /// Guarded estimator state shared across request handlers.
  inner:    Mutex<UniquesInner>,
}

/// Owned estimator snapshot used for postcard persistence.
#[derive(Serialize, Deserialize)]
struct PersistedUniques {
  /// Rotation period key the snapshot belongs to.
  salt_key: String,
  /// Salt mixed into visitor hashes.
  salt:     String,
  /// `HyperLogLog` sketch holding salted visitor hashes.
  hll:      VisitorSketch,
}

/// Borrowed estimator snapshot serialized without cloning sketch state.
#[derive(Serialize)]
struct PersistedUniquesRef<'state> {
  /// Rotation period key the snapshot belongs to.
  salt_key: &'state str,
  /// Salt mixed into visitor hashes.
  salt:     &'state str,
  /// `HyperLogLog` sketch holding salted visitor hashes.
  hll:      &'state VisitorSketch,
}

/// Live estimator state behind the shared mutex.
struct UniquesInner {
  /// Rotation period key the current salt belongs to.
  salt_key: String,
  /// Salt mixed into visitor hashes.
  salt:     String,
  /// `HyperLogLog` sketch holding salted visitor hashes.
  hll:      VisitorSketch,
}

impl UniquesInner {
  /// Resets salt and sketch when the rotation period has rolled over.
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
  #[inline]
  #[must_use]
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
  #[inline]
  pub fn add(&self, ip: &str, user_agent: &str) {
    self.rotate_if_needed();
    let mut inner = self.inner.lock();

    let visitor_hash = hash_visitor(ip, user_agent, &inner.salt);
    inner.hll.insert(Element::from_hashed(visitor_hash));
  }

  /// Returns the current estimated unique visitor count.
  #[inline]
  pub fn estimate(&self) -> f64 {
    self.rotate_if_needed();
    let inner = self.inner.lock();
    #[expect(
      clippy::cast_precision_loss,
      clippy::as_conversions,
      reason = "HyperLogLog counts stay far below 2^53 so the f64 estimate is \
                exact for every reachable cardinality"
    )]
    let estimate = inner.hll.estimate() as f64;
    estimate
  }

  /// Returns a serialized snapshot without holding the lock across I/O.
  #[inline]
  fn snapshot(&self) -> Result<Vec<u8>, UniqueStateError> {
    self.rotate_if_needed();
    let inner = self.inner.lock();
    let persisted = PersistedUniquesRef {
      salt_key: &inner.salt_key,
      salt:     &inner.salt,
      hll:      &inner.hll,
    };
    let data =
      postcard::to_stdvec(&persisted).map_err(UniqueStateError::Serialize)?;
    drop(inner);
    Ok(data)
  }
  /// Rotates salt and sketch when the rotation period has rolled over.
  fn rotate_if_needed(&self) {
    let current_key = salt_key(Timestamp::now(), self.rotation);
    let mut inner = self.inner.lock();
    if current_key != inner.salt_key {
      inner.salt_key = current_key;
      inner.salt = generate_salt(&inner.salt_key);
      inner.hll = VisitorSketch::default();
    }
  }

  /// Loads persisted state when it belongs to the current rotation period.
  ///
  /// # Errors
  ///
  /// Returns an error when the state file cannot be read or deserialized.
  #[inline]
  #[expect(
    clippy::significant_drop_tightening,
    reason = "guard must cover replace plus expiry check so a concurrent \
              rotation cannot interleave between them"
  )]
  pub async fn load(&self, path: &Path) -> Result<(), UniqueStateError> {
    let data = match fs::read(path).await {
      Ok(data) => data,
      Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
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
  ///
  /// # Errors
  ///
  /// Returns an error when the state file cannot be written.
  #[inline]
  pub async fn save(&self, path: &Path) -> Result<(), UniqueStateError> {
    let data = self.snapshot()?;

    if let Some(parent) = parent_dir(path) {
      fs::create_dir_all(parent)
        .await
        .map_err(UniqueStateError::Write)?;
    }

    write_atomic(path, &data).await
  }

  /// Returns the active salt for tests that verify state restoration.
  #[cfg(test)]
  #[inline]
  pub fn current_salt(&self) -> String {
    self.inner.lock().salt.clone()
  }
}

/// Persists estimator state through an atomic temp file rename.
async fn write_atomic(
  path: &Path,
  data: &[u8],
) -> Result<(), UniqueStateError> {
  let temp_path = tmp_path(path);
  if let Err(err) = fs::write(&temp_path, data).await {
    drop(fs::remove_file(&temp_path).await);
    return Err(UniqueStateError::Write(err));
  }
  if let Err(err) = fs::rename(&temp_path, path).await {
    drop(fs::remove_file(&temp_path).await);
    return Err(UniqueStateError::Write(err));
  }
  Ok(())
}

/// Returns the parent directory unless the path is a bare filename.
fn parent_dir(path: &Path) -> Option<&Path> {
  path
    .parent()
    .filter(|parent| !parent.as_os_str().is_empty())
}

/// Returns the temporary path used for atomic state writes.
fn tmp_path(path: &Path) -> PathBuf {
  let mut name = path.as_os_str().to_owned();
  name.push(".tmp");
  PathBuf::from(name)
}

/// Returns the rotation period key for the given timestamp.
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

/// Generates a random hex salt bound to the given rotation period key.
fn generate_salt(key: &str) -> String {
  let mut bytes = [0_u8; 32];
  rand::rng().fill_bytes(&mut bytes);
  let mut hasher = Sha256::new();
  hasher.update(key.as_bytes());
  hasher.update(bytes);
  HEXLOWER.encode(&hasher.finalize())
}

/// Hashes visitor identity with the active salt for sketch insertion.
fn hash_visitor(ip: &str, user_agent: &str, salt: &str) -> u64 {
  let digest = Sha256::hash(format!("{ip}|{user_agent}|{salt}").as_bytes());
  #[expect(
    clippy::expect_used,
    reason = "SHA-256 always yields 32 bytes so the eight-byte prefix exists"
  )]
  let bytes = digest[..8]
    .try_into()
    .expect("SHA-256 has at least eight bytes");
  #[expect(
    clippy::little_endian_bytes,
    reason = "persisted sketch hashes use little-endian order so changing it \
              would break visitor identity"
  )]
  let hash = u64::from_le_bytes(bytes);
  hash
}
#[cfg(test)]
mod tests {
  use anyhow::Result;

  use super::*;

  #[expect(
    clippy::float_cmp,
    reason = "same estimator must return identical estimate for repeated \
              observation"
  )]
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
  #[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions must panic to fail the test"
  )]
  async fn persists_current_period_state() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("hll.state");
    let estimator = UniquesEstimator::new(SaltRotation::Daily);

    estimator.add("192.0.2.1", "ua-a");
    estimator.save(&path).await?;

    let restored = UniquesEstimator::new(SaltRotation::Daily);
    restored.load(&path).await?;

    assert!(restored.estimate() >= 1.0_f64);
    assert_eq!(restored.current_salt(), estimator.current_salt());
    Ok(())
  }

  #[test]
  fn generates_unique_salts_per_period() {
    assert_ne!(generate_salt("2026-09-18"), generate_salt("2026-09-18"));
  }

  #[tokio::test]
  #[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions must panic to fail the test"
  )]
  async fn saves_without_leaving_temp_file() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("hll.state");
    let estimator = UniquesEstimator::new(SaltRotation::Daily);

    estimator.save(&path).await?;

    assert!(path.exists());
    assert!(!tmp_path(&path).exists());
    Ok(())
  }
}
