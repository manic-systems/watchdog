use std::collections::HashSet;

use parking_lot::RwLock;

/// Thread-safe set that accepts only a bounded number of distinct values.
#[derive(Debug)]
pub struct BoundedRegistry {
  /// Accepted values, guarded for shared access.
  entries:     RwLock<HashSet<String>>,
  /// Maximum number of distinct values kept.
  max_entries: usize,
}

impl BoundedRegistry {
  /// Creates an empty registry with the given maximum cardinality.
  #[inline]
  #[must_use]
  pub fn new(max_entries: usize) -> Self {
    Self {
      entries: RwLock::new(HashSet::with_capacity(max_entries)),
      max_entries,
    }
  }

  /// Adds a value, returning `false` when the registry is already full.
  #[inline]
  pub fn add(&self, value: &str) -> bool {
    if self.entries.read().contains(value) {
      return true;
    }

    let mut entries = self.entries.write();
    if entries.contains(value) {
      return true;
    }

    if entries.len() >= self.max_entries {
      return false;
    }

    entries.insert(value.to_owned());
    true
  }

  /// Returns the number of distinct values accepted so far.
  #[inline]
  pub fn count(&self) -> usize {
    self.entries.read().len()
  }

  /// Returns whether a value has already been accepted by the registry.
  #[inline]
  pub fn contains(&self, value: &str) -> bool {
    self.entries.read().contains(value)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn enforces_cardinality_limit() {
    let registry = BoundedRegistry::new(1);

    assert!(registry.add("/a"));
    assert!(registry.add("/a"));
    assert!(!registry.add("/b"));
    assert_eq!(registry.count(), 1);
  }
}
