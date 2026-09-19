use std::net::{Ipv4Addr, Ipv6Addr};

use addr::parse_domain_name;
use psl2::lookup;
use url::{Host, Url};

use crate::{config::PathConfig, limits::MAX_PATH_LEN};

/// Normalizes request paths into bounded, low-cardinality metric labels.
#[derive(Debug, Clone)]
pub struct PathNormalizer {
  /// Active path normalization rules.
  config: PathConfig,
}

impl PathNormalizer {
  /// Creates a path normalizer using the configured normalization rules.
  #[must_use]
  #[inline]
  pub const fn new(config: PathConfig) -> Self {
    Self { config }
  }

  /// Normalizes an input path into a stable label value.
  #[must_use]
  #[inline]
  pub fn normalize(&self, input: &str) -> String {
    if input.is_empty() || input.len() > MAX_PATH_LEN {
      return "/".to_owned();
    }

    let (path_query, fragment_suffix) = input
      .split_once('#')
      .map_or((input, None), |(prefix, suffix)| (prefix, Some(suffix)));

    let (path, query_suffix) = path_query
      .split_once('?')
      .map_or((path_query, None), |(prefix, suffix)| {
        (prefix, Some(suffix))
      });

    let had_trailing_slash = path.ends_with('/') && path != "/";

    let mut segments = Vec::new();
    for segment in path.split('/') {
      match segment {
        "" | "." => {},
        ".." => {
          segments.pop();
        },
        owned => segments.push(owned.to_owned()),
      }
    }

    if self.config.collapse_numeric_segments {
      for segment in &mut segments {
        if segment.bytes().all(|byte| byte.is_ascii_digit()) {
          segment.clone_from(&":id".to_owned());
        }
      }
    }

    if self.config.max_segments > 0 && segments.len() > self.config.max_segments
    {
      segments.truncate(self.config.max_segments);
    }

    let mut normalized = format!("/{}", segments.join("/"));
    if !self.config.normalize_trailing_slash
      && had_trailing_slash
      && !segments.is_empty()
    {
      normalized.push('/');
    }

    if !self.config.strip_query
      && let Some(query) = query_suffix
    {
      normalized.push('?');
      normalized.push_str(query);
    }

    if !self.config.strip_fragment
      && let Some(fragment) = fragment_suffix
    {
      normalized.push('#');
      normalized.push_str(fragment);
    }

    normalized
  }
}

/// Extracts a low-cardinality external referrer domain or an internal/direct
/// label.
#[must_use]
#[inline]
pub fn extract_referrer_domain(
  referrer: &str,
  site_domain: &str,
) -> Option<String> {
  let host = normalized_referrer_host(referrer, site_domain)?;

  if host.classification != ReferrerHostClassification::External {
    return Some(host.classification.as_label().to_owned());
  }

  parse_domain_name(&host.hostname)
    .ok()
    .and_then(|domain| lookup(domain.as_str()))
    .and_then(|domain| domain.registrable_domain().map(str::to_owned))
    .or_else(|| Some("other".to_owned()))
}

/// Extracts a sanitized external referrer URL or an internal/direct label.
#[must_use]
#[inline]
pub fn extract_referrer_url(
  referrer: &str,
  site_domain: &str,
) -> Option<String> {
  let host = normalized_referrer_host(referrer, site_domain)?;

  if host.classification != ReferrerHostClassification::External {
    return Some(host.classification.as_label().to_owned());
  }

  let mut url = Url::parse(referrer).ok()?;
  if url.set_username("").is_err() {
    return None;
  }
  if url.set_password(None).is_err() {
    return None;
  }
  url.set_fragment(None);
  Some(url.to_string())
}

/// Classifies a referrer host as direct, internal, or external.
#[derive(Debug, PartialEq, Eq)]
enum ReferrerHostClassification {
  /// Missing referrer, treated as direct traffic.
  Direct,
  /// Loopback, private, or link-local host.
  Internal,
  /// Any other routable host.
  External,
}

impl ReferrerHostClassification {
  /// Returns the metric label for this host classification.
  const fn as_label(&self) -> &'static str {
    match *self {
      Self::Direct => "direct",
      Self::Internal => "internal",
      Self::External => "external",
    }
  }
}

/// Parsed referrer host with its traffic classification.
struct ReferrerHost {
  /// Lowercased hostname without trailing dot.
  hostname:       String,
  /// Traffic classification for the hostname.
  classification: ReferrerHostClassification,
}

/// Returns the parsed referrer host for a raw referrer string.
#[inline]
fn normalized_referrer_host(
  referrer: &str,
  site_domain: &str,
) -> Option<ReferrerHost> {
  if referrer.is_empty() {
    return Some(ReferrerHost {
      hostname:       String::new(),
      classification: ReferrerHostClassification::Direct,
    });
  }

  let url = Url::parse(referrer).ok()?;
  let hostname = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
  if hostname.is_empty() {
    return None;
  }

  let internal = match url.host()? {
    Host::Domain(_) => {
      hostname == "localhost" || hostname.ends_with(".localhost")
    },
    Host::Ipv4(address) => is_internal_ipv4(address),
    Host::Ipv6(address) => is_internal_ipv6(address),
  };

  if internal {
    return Some(ReferrerHost {
      hostname,
      classification: ReferrerHostClassification::Internal,
    });
  }

  let trimmed_site = site_domain.trim_end_matches('.').to_ascii_lowercase();
  if hostname == trimmed_site || hostname.ends_with(&format!(".{trimmed_site}"))
  {
    return Some(ReferrerHost {
      hostname,
      classification: ReferrerHostClassification::Direct,
    });
  }

  Some(ReferrerHost {
    hostname,
    classification: ReferrerHostClassification::External,
  })
}

/// Returns true for private, loopback, or link-local IPv4 addresses.
const fn is_internal_ipv4(ip: Ipv4Addr) -> bool {
  ip.is_private() || ip.is_loopback() || ip.is_link_local()
}

/// Returns true for loopback, private, or link-local IPv6 addresses.
#[inline]
fn is_internal_ipv6(ip: Ipv6Addr) -> bool {
  ip.is_loopback()
    || ip.is_unique_local()
    || ip.is_unicast_link_local()
    || ip.to_ipv4_mapped().is_some_and(is_internal_ipv4)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::config::PathConfig;

  #[test]
  fn normalizes_paths() {
    let normalizer = PathNormalizer::new(PathConfig::default());

    assert_eq!(normalizer.normalize("docs/123/?q=1#top"), "/docs/:id");
    assert_eq!(normalizer.normalize("/a/./b/../c"), "/a/c");
    assert_eq!(normalizer.normalize(""), "/");
  }

  #[test]
  fn preserves_trailing_slash_when_configured() {
    let normalizer = PathNormalizer::new(PathConfig {
      normalize_trailing_slash: false,
      ..PathConfig::default()
    });

    assert_eq!(normalizer.normalize("/docs/"), "/docs/");
  }

  #[test]
  fn extracts_effective_referrer_domain() {
    assert_eq!(
      extract_referrer_domain(
        "https://news.ycombinator.com/item?id=1",
        "example.com"
      ),
      Some("ycombinator.com".to_owned())
    );
    assert_eq!(
      extract_referrer_domain("https://blog.example.com/post", "example.com"),
      Some("direct".to_owned())
    );
    assert_eq!(
      extract_referrer_domain("http://127.0.0.1/test", "example.com"),
      Some("internal".to_owned())
    );
    assert_eq!(
      extract_referrer_domain("https://app.localhost/test", "example.com"),
      Some("internal".to_owned())
    );
    assert_eq!(
      extract_referrer_domain("https://localhost.evil.com/test", "example.com"),
      Some("evil.com".to_owned())
    );
  }
}
