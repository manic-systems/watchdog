use std::{
  collections::HashSet,
  sync::atomic::{AtomicUsize, Ordering},
};

use parking_lot::RwLock;

/// Thread-safe set that accepts only a bounded number of distinct values.
#[derive(Debug)]
pub struct BoundedRegistry {
  entries:     RwLock<HashSet<String>>,
  max_entries: usize,
  overflows:   AtomicUsize,
}

impl BoundedRegistry {
  /// Creates an empty registry with the given maximum cardinality.
  pub fn new(max_entries: usize) -> Self {
    Self {
      entries: RwLock::new(HashSet::with_capacity(max_entries)),
      max_entries,
      overflows: AtomicUsize::new(0),
    }
  }

  /// Adds a value, returning `false` when the registry is already full.
  pub fn add(&self, value: &str) -> bool {
    if self.entries.read().contains(value) {
      return true;
    }

    let mut entries = self.entries.write();
    if entries.contains(value) {
      return true;
    }

    if entries.len() >= self.max_entries {
      self.overflows.fetch_add(1, Ordering::Relaxed);
      return false;
    }

    entries.insert(value.to_owned());
    true
  }

  /// Returns the number of distinct values accepted so far.
  pub fn count(&self) -> usize {
    self.entries.read().len()
  }

  /// Returns how many distinct values were rejected after the registry filled.
  pub fn overflow_count(&self) -> usize {
    self.overflows.load(Ordering::Relaxed)
  }

  /// Returns whether a value has already been accepted by the registry.
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
    assert_eq!(registry.overflow_count(), 1);
  }
}
