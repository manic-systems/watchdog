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
  inner:  Arc<Mutex<HashMap<IpAddr, Window>>>,
  limit:  NonZeroU32,
  window: Duration,
}

#[derive(Debug)]
struct Window {
  count: u32,
  start: Instant,
}

/// Upper bound on tracked addresses; expired entries are pruned past it.
const MAX_TRACKED_IPS: usize = 100_000;

impl IpRateLimiter {
  /// Creates a limiter allowing `limit` requests per minute per IP.
  pub fn per_minute(limit: NonZeroU32) -> Self {
    Self::with_window(limit, Duration::from_secs(60))
  }

  fn with_window(limit: NonZeroU32, window: Duration) -> Self {
    Self {
      inner: Arc::new(Mutex::new(HashMap::new())),
      limit,
      window,
    }
  }

  /// Returns true when a request from `ip` fits its current window.
  pub fn check(&self, ip: IpAddr) -> bool {
    let mut inner = self.inner.lock();
    if inner.len() > MAX_TRACKED_IPS {
      let window = self.window;
      inner.retain(|_, entry| entry.start.elapsed() < window);
    }

    match inner.get_mut(&ip) {
      Some(entry) if entry.start.elapsed() < self.window => {
        if entry.count < self.limit.get() {
          entry.count += 1;
          true
        } else {
          false
        }
      },
      _ => {
        inner.insert(ip, Window {
          count: 1,
          start: Instant::now(),
        });
        true
      },
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn limit(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test limit must be nonzero")
  }

  #[test]
  fn enforces_per_ip_quota() {
    let limiter = IpRateLimiter::with_window(limit(2), Duration::from_secs(60));
    let addr: IpAddr = "192.0.2.1".parse().unwrap();

    assert!(limiter.check(addr));
    assert!(limiter.check(addr));
    assert!(!limiter.check(addr));
  }

  #[test]
  fn quotas_are_independent_per_ip() {
    let limiter = IpRateLimiter::with_window(limit(1), Duration::from_secs(60));
    let exhausted: IpAddr = "192.0.2.1".parse().unwrap();
    let fresh: IpAddr = "192.0.2.2".parse().unwrap();

    assert!(limiter.check(exhausted));
    assert!(!limiter.check(exhausted));
    assert!(limiter.check(fresh));
  }

  #[test]
  fn window_expiry_restores_quota() {
    let limiter =
      IpRateLimiter::with_window(limit(1), Duration::from_millis(20));
    let addr: IpAddr = "192.0.2.1".parse().unwrap();

    assert!(limiter.check(addr));
    assert!(!limiter.check(addr));
    std::thread::sleep(Duration::from_millis(30));
    assert!(limiter.check(addr));
  }

  #[test]
  fn prunes_expired_entries_when_full() {
    let limiter =
      IpRateLimiter::with_window(limit(10), Duration::from_secs(60));
    let stale = Instant::now() - Duration::from_secs(61);
    {
      let mut inner = limiter.inner.lock();
      for index in 0..=MAX_TRACKED_IPS as u32 {
        let addr = IpAddr::from([
          10,
          (index >> 16) as u8,
          (index >> 8) as u8,
          index as u8,
        ]);
        inner.insert(addr, Window {
          count: 1,
          start: stale,
        });
      }
    }

    let fresh: IpAddr = "192.0.2.1".parse().unwrap();
    assert!(limiter.check(fresh));
    assert!(limiter.inner.lock().len() < MAX_TRACKED_IPS);
  }
}
