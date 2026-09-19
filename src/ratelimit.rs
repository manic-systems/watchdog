use std::{
  collections::HashMap,
  net::IpAddr,
  num::NonZeroU32,
  sync::Arc,
  time::{Duration, Instant},
};

use parking_lot::Mutex;

/// Fixed-window rate limiter with an independent quota per client IP.
///
/// One client cannot consume the budget of others. Aggregate flood
/// protection is intentionally left to edge proxies.
#[derive(Debug, Clone)]
pub struct IpRateLimiter {
  /// Shared per address windows behind one mutex.
  inner:  Arc<Mutex<IpWindows>>,
  /// Allowed requests per window for every address.
  limit:  NonZeroU32,
  /// Length of one fixed window.
  window: Duration,
}

/// Mutable per address windows plus the next prune time.
#[derive(Debug)]
struct IpWindows {
  /// Live windows keyed by client address.
  entries:      HashMap<IpAddr, Window>,
  /// Earliest time a full prune may run again.
  next_cleanup: Instant,
}

/// One fixed window counter.
#[derive(Debug)]
struct Window {
  /// Requests seen in the current window.
  count: u32,
  /// Time the current window started.
  start: Instant,
}

/// Upper bound on tracked addresses; expired entries are pruned past it.
const MAX_TRACKED_IPS: usize = 100_000;

impl IpRateLimiter {
  /// Creates a limiter allowing `limit` requests per minute per IP.
  #[inline]
  #[must_use]
  pub fn per_minute(limit: NonZeroU32) -> Self {
    Self::with_window(limit, Duration::from_mins(1))
  }

  /// Builds a limiter with an explicit window.
  fn with_window(limit: NonZeroU32, window: Duration) -> Self {
    Self {
      inner: Arc::new(Mutex::new(IpWindows {
        entries:      HashMap::new(),
        next_cleanup: Instant::now(),
      })),
      limit,
      window,
    }
  }

  /// Returns true when a request from `ip` fits its current window.
  #[must_use]
  #[inline]
  pub fn check(&self, ip: IpAddr) -> bool {
    let mut inner = self.inner.lock();
    let now = Instant::now();

    if let Some(entry) = inner.entries.get_mut(&ip) {
      if now.duration_since(entry.start) >= self.window {
        entry.count = 0;
        entry.start = now;
      }

      if entry.count >= self.limit.get() {
        return false;
      }

      entry.count += 1;
      return true;
    }

    if inner.entries.len() >= MAX_TRACKED_IPS && now >= inner.next_cleanup {
      inner
        .entries
        .retain(|_, entry| now.duration_since(entry.start) < self.window);
      inner.next_cleanup = now + Duration::from_secs(1);
    }

    if inner.entries.len() >= MAX_TRACKED_IPS {
      return false;
    }

    inner.entries.insert(ip, Window {
      count: 1,
      start: now,
    });
    true
  }
}

#[cfg(test)]
mod tests {
  use std::thread::sleep;

  use super::*;

  const fn limit(value: u32) -> NonZeroU32 {
    match NonZeroU32::new(value) {
      Some(limit) => limit,
      None => NonZeroU32::MIN,
    }
  }

  #[test]
  fn enforces_per_ip_quota() {
    let limiter = IpRateLimiter::with_window(limit(2), Duration::from_mins(1));
    let addr: IpAddr = IpAddr::from([192_u8, 0_u8, 2_u8, 1_u8]);

    assert!(limiter.check(addr));
    assert!(limiter.check(addr));
    assert!(!limiter.check(addr));
  }

  #[test]
  fn quotas_are_independent_per_ip() {
    let limiter = IpRateLimiter::with_window(limit(1), Duration::from_mins(1));
    let exhausted: IpAddr = IpAddr::from([192_u8, 0_u8, 2_u8, 1_u8]);
    let fresh: IpAddr = IpAddr::from([192_u8, 0_u8, 2_u8, 2_u8]);
    assert!(limiter.check(exhausted));
    assert!(!limiter.check(exhausted));
    assert!(limiter.check(fresh));
  }

  #[test]
  fn window_expiry_restores_quota() {
    let limiter =
      IpRateLimiter::with_window(limit(1), Duration::from_millis(20));
    let addr: IpAddr = IpAddr::from([192_u8, 0_u8, 2_u8, 1_u8]);

    assert!(limiter.check(addr));
    assert!(!limiter.check(addr));
    sleep(Duration::from_millis(30));
    assert!(limiter.check(addr));
  }

  #[test]
  fn prunes_expired_entries_when_full() {
    let limiter = IpRateLimiter::with_window(limit(10), Duration::from_mins(1));
    let stale = Instant::now()
      .checked_sub(Duration::from_secs(61))
      .unwrap_or_else(Instant::now);
    let bound = u32::try_from(MAX_TRACKED_IPS).unwrap_or(u32::MAX);
    {
      let mut inner = limiter.inner.lock();
      for index in 0..=bound {
        #[expect(
          clippy::little_endian_bytes,
          reason = "test-only address packing, never persisted or hashed"
        )]
        let octets = index.to_le_bytes();
        let addr = IpAddr::from([10_u8, octets[2], octets[1], octets[0]]);
        inner.entries.insert(addr, Window {
          count: 1,
          start: stale,
        });
      }
    }

    let fresh: IpAddr = IpAddr::from([192_u8, 0_u8, 2_u8, 1_u8]);
    assert!(limiter.check(fresh));
    assert!(limiter.inner.lock().entries.len() < MAX_TRACKED_IPS);
  }
}
