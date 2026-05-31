# Configuration

Watchdog configuration is loaded in this order, from lowest to highest
precedence:

1. Built-in defaults
2. TOML configuration file
3. Environment variables
4. Command-line flags

## Configuration File

By default, Watchdog looks for these TOML files:

- `./config.toml`
- `/etc/watchdog/config.toml`

Specify a custom path with `--config`:

```bash
watchdog --config /path/to/config.toml
```

See [config.example.toml](../config.example.toml) for a complete example.

## Minimal Config

Only `site.domains` is required. All limits and server paths have conservative
defaults.

```toml
[site]
domains = ["example.com"]
```

## Environment Variables

Environment variables use the `WATCHDOG_` prefix. Nested fields use double
underscores so field names can keep their TOML underscores unambiguously.

```bash
export WATCHDOG_SERVER__LISTEN_ADDR="127.0.0.1:8080"
export WATCHDOG_SITE__SAMPLING=1.0
export WATCHDOG_SITE__COLLECT__DEVICE=true
export WATCHDOG_LIMITS__MAX_PATHS=10000
export WATCHDOG_SECURITY__METRICS_AUTH__PASSWORD="secret"
```

Prefer TOML for arrays such as `site.domains` and `security.trusted_proxies`.

## Command-Line Flags

The CLI intentionally exposes only operational overrides:

```bash
watchdog --listen-addr :9090
watchdog --metrics-path /prometheus/metrics
watchdog --ingestion-path /api/v1/event
watchdog --config prod.toml --listen-addr :9090
```

Available flags:

- `--config <path>`
- `--listen-addr <addr>`
- `--metrics-path <path>`
- `--ingestion-path <path>`

## Reference

### Site

```toml
[site]
domains = ["example.com", "blog.example.com"]
salt_rotation = "daily" # "daily", "hourly", or omit to disable uniques
sampling = 1.0
custom_events = ["signup", "purchase"]
```

### Collection

```toml
[site.collect]
pageviews = true
sessions = true
engagement = true
country = false
device = true
browser = false
os = false
screen = false
referrer = "domain" # "off", "domain", or "url"
acquisition = false
properties = false
domain = false
```

`country = true` currently emits the `country="unknown"` label. It is retained
as a stable configuration surface for future GeoIP enrichment.

`sessions` counts client-reported session starts without storing or exporting a
session identifier. `engagement` enables aggregate engagement seconds and scroll
depth buckets. `acquisition` adds UTM labels, click-id parameter names, and
referrer-source labels behind `limits.max_dimension_values`. `properties`
records bounded custom properties as `key`/`value` labels.

### Path Normalization

```toml
[site.path]
strip_query = true
strip_fragment = true
collapse_numeric_segments = true
max_segments = 5
normalize_trailing_slash = true
```

### Limits

```toml
[limits]
max_paths = 10000
max_sources = 500
max_custom_events = 100
max_dimension_values = 1000
max_property_keys = 50
max_property_values = 500
max_events_per_minute = 10000
max_metrics_per_minute = 60

[limits.device_breakpoints]
mobile = 768
tablet = 1024
```

### Security

```toml
[security]
trusted_proxies = ["127.0.0.1", "10.0.0.0/8"]

[security.cors]
enabled = false
allowed_origins = ["*"]

[security.metrics_auth]
enabled = false
username = "admin"
password = "changeme"
```

Only requests arriving from `trusted_proxies` may use `X-Forwarded-For` or
`X-Real-IP` for visitor estimation.

### Server

```toml
[server]
listen_addr = "127.0.0.1:8080"
metrics_path = "/metrics"
ingestion_path = "/api/event"
state_path = "/var/lib/watchdog/hll.state"
```

## Systemd

```ini
[Service]
Environment="WATCHDOG_SERVER__LISTEN_ADDR=127.0.0.1:8080"
Environment="WATCHDOG_SECURITY__METRICS_AUTH__PASSWORD=secret"
ExecStart=/usr/local/bin/watchdog --config /etc/watchdog/config.toml
```

## NixOS

```nix
{
  services.watchdog = {
    enable = true;
    settings = {
      site.domains = ["example.com"];
      server.listen_addr = "127.0.0.1:8080";
      limits.max_paths = 10000;
    };
  };
}
```

The NixOS module serializes `settings` as TOML and defaults
`server.state_path` to the module `stateDir`.

## Validation

Invalid configuration fails startup with a clear error. Common failures include:

- Missing `site.domains`
- `site.sampling` outside `0.0..=1.0`
- Zero cardinality limits
- Enabled CORS without `allowed_origins`
- Enabled metrics auth without username or password
- Endpoint paths that do not start with `/`
