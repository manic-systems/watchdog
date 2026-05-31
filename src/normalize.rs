use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use addr::parse_domain_name;
use url::Url;

use crate::{config::PathConfig, limits::MAX_PATH_LEN};

#[derive(Debug, Clone)]
pub struct PathNormalizer {
  config: PathConfig,
}

impl PathNormalizer {
  pub fn new(config: PathConfig) -> Self {
    Self { config }
  }

  pub fn normalize(&self, input: &str) -> String {
    if input.is_empty() || input.len() > MAX_PATH_LEN {
      return "/".to_owned();
    }

    let mut path = input;
    if self.config.strip_query {
      path = path.split_once('?').map_or(path, |(path, _)| path);
    }
    if self.config.strip_fragment {
      path = path.split_once('#').map_or(path, |(path, _)| path);
    }

    let had_trailing_slash = path.ends_with('/') && path != "/";
    let prefixed;
    if !path.starts_with('/') {
      prefixed = format!("/{path}");
      path = &prefixed;
    }

    let mut segments = Vec::new();
    for segment in path.split('/') {
      match segment {
        "" | "." => {},
        ".." => {
          segments.pop();
        },
        segment => segments.push(segment.to_owned()),
      }
    }

    if self.config.collapse_numeric_segments {
      for segment in &mut segments {
        if segment.bytes().all(|byte| byte.is_ascii_digit()) {
          *segment = ":id".to_owned();
        }
      }
    }

    if self.config.max_segments > 0 && segments.len() > self.config.max_segments
    {
      segments.truncate(self.config.max_segments);
    }

    if segments.is_empty() {
      return "/".to_owned();
    }

    let mut normalized = format!("/{}", segments.join("/"));
    if !self.config.normalize_trailing_slash && had_trailing_slash {
      normalized.push('/');
    }
    normalized
  }
}

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
    .and_then(|domain| domain.root().map(str::to_owned))
    .or_else(|| Some("other".to_owned()))
}

pub fn extract_referrer_url(
  referrer: &str,
  site_domain: &str,
) -> Option<String> {
  let host = normalized_referrer_host(referrer, site_domain)?;

  if host.classification != ReferrerHostClassification::External {
    return Some(host.classification.as_label().to_owned());
  }

  let mut url = Url::parse(referrer).ok()?;
  let _ = url.set_username("");
  let _ = url.set_password(None);
  url.set_fragment(None);
  Some(url.to_string())
}

#[derive(Debug, PartialEq, Eq)]
enum ReferrerHostClassification {
  Direct,
  Internal,
  External,
}

impl ReferrerHostClassification {
  fn as_label(&self) -> &'static str {
    match self {
      Self::Direct => "direct",
      Self::Internal => "internal",
      Self::External => "external",
    }
  }
}

struct ReferrerHost {
  hostname: String,
  classification: ReferrerHostClassification,
}

fn normalized_referrer_host(
  referrer: &str,
  site_domain: &str,
) -> Option<ReferrerHost> {
  if referrer.is_empty() {
    return Some(ReferrerHost {
      hostname: String::new(),
      classification: ReferrerHostClassification::Direct,
    });
  }

  let url = Url::parse(referrer).ok()?;
  let hostname = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
  if hostname.is_empty() {
    return None;
  }

  if is_internal_host(&hostname) {
    return Some(ReferrerHost {
      hostname,
      classification: ReferrerHostClassification::Internal,
    });
  }

  let site_domain = site_domain.trim_end_matches('.').to_ascii_lowercase();
  if hostname == site_domain || hostname.ends_with(&format!(".{site_domain}")) {
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

fn is_internal_host(hostname: &str) -> bool {
  if hostname == "localhost" || hostname.starts_with("localhost.") {
    return true;
  }

  match hostname.parse::<IpAddr>() {
    Ok(IpAddr::V4(ip)) => is_internal_ipv4(ip),
    Ok(IpAddr::V6(ip)) => is_internal_ipv6(ip),
    Err(_) => false,
  }
}

fn is_internal_ipv4(ip: Ipv4Addr) -> bool {
  ip.is_private() || ip.is_loopback() || ip.is_link_local()
}

fn is_internal_ipv6(ip: Ipv6Addr) -> bool {
  ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
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
  }
}
