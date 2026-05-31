# Watchdog

Watchdog is a privacy-first analytics system with fully declarative
configuration and little to no resource footprint designed to aggregate web
traffic data and exports it as Prometheus metrics. Unlike traditional analytics
platforms, Watchdog stores no raw events, uses no persistent user identifiers,
and enforces bounded cardinality by design.

## Features

Watchdog is **privacy-first** in design. There are no cookies, no `localStorage`
or browser fingerprinting. Daily salt rotation prevents cross-day visitor
correlation, and there is no raw event storage. We only aggregate metrics; you
visualise them as you see fit.

Other noteworthy features:

- Multi-site analytics with optional domain tracking
- Bounded cardinality prevents metric explosion
- Rate limiting and DoS protection
- Graceful shutdown with state persistence
- IPv6 support with proper proxy header handling

The privacy design choices of Watchdog are a little more intricate. See the
[privacy section](#privacy) for more details on what "guarantees" Watchdog has
to offer.

## Quick Start

### NixOS

The recommended way of using Watchdog is consuming the NixOS module. It handles
everything from dependencies to environment setup and Systemd configuration.
Simply import the NixOS module provided by this flake, and enable
`services.watchdog`:

```nix
{inputs, ...}: {
  imports = [inputs.watchdog.nixosModules.default];

  services.watchdog = {
    enable = true;
    settings = {
      site.domains = ["example.com"];
      server.listen_addr = "127.0.0.1:8080";
    };
  };
}
```

The `settings` option is freeform, meaning it'll be serialized to the TOML
configuration automatically.

### Systemd

On non-NixOS distributions, you may build Watchdog with `cargo build` and copy
it somewhere that's in your `PATH`. Usually this is `/usr/local/bin` for system
installations:

```bash
# Build
$ cargo build --release --locked
$ sudo install -Dm755 target/release/watchdog /usr/local/bin/watchdog

# Install service
$ sudo install -Dm644 contrib/systemd/watchdog.service /etc/systemd/system/
$ sudo systemctl daemon-reload
$ sudo systemctl enable --now watchdog
```

### Binary

You may also run the binary from any path by simply building and running it, but
it is generally not very advisable. You may consider something like `screen` to
background the Watchdog process. Though, Systemd is the only supported
installation mechanism.

```bash
# Build
$ cargo build --release --locked

# Run
$ ./target/release/watchdog --config config.toml
```

## Configuration

[configuration reference]: docs/configuration.md

Watchdog currently supports configuration via TOML file, environment variables
or command-line flags. You may find a more complete reference in the
[configuration reference] document.

To get started, create `config.toml`:

```toml
[site]
# Single-site analytics
domains = ["example.com"]

# Or multi-site analytics
# domains = ["example.com", "blog.example.com", "shop.example.com"]

salt_rotation = "daily"

[site.collect]
pageviews = true
sessions = true
engagement = true
device = true
browser = false
os = false
screen = false
referrer = "domain"
acquisition = false
properties = false
domain = false # Set to true for multi-site analytics

[limits]
max_paths = 10000
max_sources = 500
max_custom_events = 100
max_dimension_values = 1000
max_property_keys = 50
max_property_values = 500
max_events_per_minute = 10000

[server]
listen_addr = "127.0.0.1:8080"
```

## Usage

### JavaScript Beacon

Similar to Plausible, Watchdog uses a JavaScript beacon to track events. In the
most basic case, you must add it to your site in a `<script>` tag to begin
collecting metrics:

```html
<script src="https://analytics.example.com/web/beacon.js" defer></script>
```

The script beacon also supports a _variety_ of configuration options via data
attributes, which you might adjust to your own needs. Some of them are described
below:

```html
<!-- Custom API endpoint -->
<script src="/web/beacon.js" data-api="/custom/endpoint" defer></script>

<!-- Track specific domain (multi-site) -->
<script src="/web/beacon.js" data-domain="example.com" defer></script>

<!-- Hash-based routing (for SPAs) -->
<script src="/web/beacon.js" data-hash-mode defer></script>

<!-- Track outbound links -->
<script src="/web/beacon.js" data-outbound-links defer></script>

<!-- Track file downloads (.pdf, .zip, .doc, etc.) -->
<script src="/web/beacon.js" data-file-downloads defer></script>

<!-- Exclude paths (comma-separated) -->
<script src="/web/beacon.js" data-exclude="/admin,/dashboard" defer></script>

<!-- Manual pageview tracking -->
<script src="/web/beacon.js" data-manual defer></script>

<!-- Allow local development traffic -->
<script src="/web/beacon.js" data-allow-localhost defer></script>

<!-- Disable engagement and scroll tracking in the beacon -->
<script src="/web/beacon.js" data-disable-engagement defer></script>

<!-- Combine multiple options -->
<script
  src="/web/beacon.js"
  data-hash-mode
  data-outbound-links
  data-file-downloads
  defer
></script>
```

You can also track custom events as follows:

```javascript
// Simple event
window.watchdog.track("signup");

// Event with custom referrer
window.watchdog.track("purchase", { referrer: "email-campaign" });

// Event with bounded custom properties. Requires site.collect.properties = true.
window.watchdog.track("purchase", { props: { plan: "pro" } });

// Manual pageview (when data-manual is set)
window.watchdog.trackPageview();

// Force pageview (bypass duplicate detection)
window.watchdog.trackPageview({ force: true });
```

### Metrics

[observability documentation]: docs/observability.md

Unlike most common solutions, Watchdog does not do any data visualisation. It
collects and aggregates metrics in a Prometheus-compatible manner at the
`/metrics` endpoint. To scrape and store the time-series data, you will need
**Prometheus**. Grafana is a common solution to _visualizing_ said data.

See the [observability documentation] for full setup guide. It is, however,
recommended to be somewhat knowledgeable in those areas before attempting to
deploy. There exists a Grafana dashboard with support for multi-host and
multi-site deployments provided in the [contrib directory](contrib/grafana/).

Core metrics include:

**Traffic metrics:**

- `web_pageviews_total{path,device,referrer,domain}` - Total pageviews. The
  `domain` label is only present if `site.collect.domain: true`
- `web_events_total{event,path,...}` - Non-pageview event counts with enabled
  dimensions
- `web_custom_events_total{event}` - Custom event counts
- `web_sessions_total{path,...}` - Client-reported session starts
- `web_engagement_seconds_total{path,...}` - Aggregate active engagement time
- `web_scroll_depth_total{path,depth,...}` - Scroll-depth reports
- `web_custom_properties_total{event,key,value}` - Bounded custom properties
- `web_daily_unique_visitors` - Estimated unique visitors (HyperLogLog)

**Cardinality metrics:**

- `web_path_overflow_total` - Paths rejected due to cardinality limit
- `web_referrer_overflow_total` - Referrers rejected due to limit
- `web_event_overflow_total` - Custom events rejected due to limit
- `web_dimension_overflow_total{dimension}` - Rich dimensions collapsed due to
  limit
- `web_blocked_requests_total{reason}` - File server requests blocked by
  security filters

**Process metrics:**

- `watchdog_build_info{version,commit,build_date}` - Build metadata
- `watchdog_start_time_seconds` - Unix timestamp of process start
- `process_*` - OS process metrics (CPU, RSS, file descriptors)

## Privacy

Privacy is a fundamental design constraint for Watchdog, and not a feature. All
personally identifiable information is discarded at ingestion. We only keep
aggregate counts. Some features worth noting.

**No Persistent Identifiers:**

- No cookies, `localStorage`, or fingerprinting
- Session starts use a per-tab `sessionStorage` marker with no visitor ID
- IP + User-Agent hashed with daily rotating salt
- Hash discarded after HLL insertion

**Data Minimization:**

- Required event fields are limited to domain and URL/path
- Optional aggregate fields include referrer, screen width, engagement time,
  scroll depth, UTM labels, coarse device labels, and bounded custom properties
- No raw event storage
- All data aggregated at ingestion

**Bounded Cardinality:**

- Configurable limits on paths, referrers, events
- Prevents unbounded metric growth
- Overflow tracked separately

**GDPR/CCPA Compliance:**

- No personal data stored
- Daily salt rotation prevents cross-day correlation
- Aggregate-only analytics

## Contributing

Watchdog was built in a very short duration to address a very specific need:
replace Plausible and replace it as soon as possible. While I've given the
appropriate amount of care to the codebase, there may be unintended bugs or
missing features that you'd like to support. In this case, you are very welcome
to create issues or PRs.

### Building

The recommended way of developing Watchdog is using the Nix flake, for a
reproducible toolchain. The default shell already provides everything you need,
so you can simply use `nix develop` or use `direnv allow` if you use Direnv.

Once you have the dependencies, the workflow is relatively simple. Build and
test with Cargo, and then with Nix to ensure packaging is correct.

```bash
# Build binary
$ cargo build --release --locked

# Run tests
$ cargo test --locked --all-targets

# Run clippy
$ cargo clippy --locked --all-targets -- -D warnings

# Build with Nix
$ nix build --builders ""
```

### Testing

There's a non-negligible test suite for Watchdog that I'm quite proud of. It
tests anything from configuration validation to path normalization and other
security features that we'd _rather not regress_. If working on Watchdog, it is
advisable that you add test cases and run the tests before submitting your
changes.

```bash
# Unit tests
$ cargo test --locked --all-targets

# Lints
$ cargo clippy --locked --all-targets -- -D warnings

# Coverage
$ cargo llvm-cov
```

## License

<!-- markdownlint-disable MD059 -->

[here]: https://interoperable-europe.ec.europa.eu/sites/default/files/custom-page/attachment/eupl_v1.2_en.pdf

This project is made available under European Union Public Licence (EUPL)
version 1.2. See [LICENSE](LICENSE) for more details on the exact conditions. An
online copy is provided [here].

<!-- markdownlint-enable MD059 -->
