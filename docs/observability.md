# Observability Setup

Watchdog exposes Prometheus-formatted metrics at `/metrics`. You need a
time-series database to scrape and store these metrics, then visualize them in
Grafana.

> [!IMPORTANT]
>
> **Why you need a time-series database:**
>
> - Watchdog exposes _current state_ (counters, gauges)
> - A TSDB _scrapes periodically_ and _stores time-series data_
> - Grafana _visualizes_ the historical data
> - Grafana cannot directly scrape Prometheus `/metrics` endpoints
>
> **Compatible databases:**
>
> - [Prometheus](#prometheus-setup),
> - [VictoriaMetrics](#victoriametrics), or any Prometheus-compatible scraper

## Prometheus Setup

### Configuring Prometheus

Create `/etc/prometheus/prometheus.yml`:

```yaml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  - job_name: "watchdog"
    static_configs:
      - targets: ["localhost:8080"]

    # Optional: scrape multiple Watchdog instances
    # static_configs:
    #   - targets:
    #       - 'watchdog-1.example.com:8080'
    #       - 'watchdog-2.example.com:8080'
    #     labels:
    #       instance: 'production'

  # Scrape Prometheus itself
  - job_name: "prometheus"
    static_configs:
      - targets: ["localhost:9090"]
```

### Verify Prometheus' health state

```bash
# Check Prometheus is running
curl http://localhost:9090/-/healthy

# Check it's scraping Watchdog
curl http://localhost:9090/api/v1/targets
```

### NixOS

Add to your NixOS configuration:

```nix
{
  services.prometheus = {
    enable = true;
    port = 9090;

    # Retention period
    retentionTime = "30d";

    scrapeConfigs = [
      {
        job_name = "watchdog";
        static_configs = [{
          targets = [ "localhost:8080" ];
        }];
      }
    ];
  };

  # Open firewall if needed
  # networking.firewall.allowedTCPPorts = [ 9090 ];
}
```

For multiple Watchdog instances:

```nix
{
  services.prometheus.scrapeConfigs = [
    {
      job_name = "watchdog";
      static_configs = [
        {
          labels.env = "production";
          targets = [
            "watchdog-1:8080"
            "watchdog-2:8080"
            "watchdog-3:8080"
          ];
        }
      ];
    }
  ];
}
```

## Grafana Setup

### NixOS

```nix
{
  services.grafana = {
    enable = true;
    settings = {
      server = {
        http_addr = "127.0.0.1";
        http_port = 3000;
      };
    };

    provision = {
      enable = true;

      datasources.settings.datasources = [{
        name = "Prometheus";
        type = "prometheus";
        url = "http://localhost:9090";  # Or "http://localhost:8428" for VictoriaMetrics
        isDefault = true;
      }];
    };
  };
}
```

### Configure Data Source (Manual)

If you're not using NixOS for provisioning, then you'll need to do provisioning
_imperatively_ from your Grafana configuration. This can be done through the
admin panel by navigating to `Configuration`, and choosing "add data source"
under `Data Sources`. Select your prometheus instance, and save it.

### Import Pre-built Dashboard

A sample Grafana dashboard is provided with support for multi-host and
multi-site configurations. Import it, configure the data source and it should
work out of the box.

If you're not using NixOS for provisioning, the dashboard _also_ needs to be
provisioned manually. Under `Dashboards`, select `Import` and provide the JSON
contents or upload the sample dashboard from `contrib/grafana/watchdog.json`.
Select your Prometheus data source and import it.

See [contrib/grafana/README.md](../contrib/grafana/README.md) for full
documentation.

## Example Queries

Once Prometheus is scraping Watchdog and Grafana is connected, you may write
your own widgets or create queries. Here are some example queries using
Prometheus query language, promql. Those are provided as examples and might not
provide everything you need. Nevertheless, use them to improve your setup at
your disposal.

If you believe you have some valuable widgets that you'd like to contribute
back, feel free!

### Top 10 Pages by Traffic

```promql
topk(10, sum by (path) (rate(web_pageviews_total[5m])))
```

### Mobile vs Desktop Split

```promql
sum by (device) (rate(web_pageviews_total[1h]))
```

### Unique Visitors

Daily rotation exposes `web_daily_unique_visitors`; hourly rotation exposes
`web_hourly_unique_visitors`.

```promql
web_daily_unique_visitors
```

### Top Referrers

```promql
topk(10, sum by (referrer) (rate(web_pageviews_total{referrer!="direct"}[1d])))
```

### Multi-Site: Traffic per Domain

```promql
sum by (domain) (rate(web_pageviews_total[1h]))
```

### Cardinality Health

```promql
# Should be near zero
rate(web_path_overflow_total[5m])
rate(web_referrer_overflow_total[5m])
rate(web_event_overflow_total[5m])
rate(web_dimension_overflow_total[5m])
rate(web_series_overflow_total[5m])
```

### Engagement

```promql
sum(rate(web_engagement_seconds_total[5m]))
sum by (depth) (rate(web_scroll_depth_total[1h]))
```

## Horizontal Scaling Considerations

When running multiple Watchdog instances:

1. **Each instance exposes its own metrics** - Prometheus scrapes all instances
2. **Prometheus aggregates automatically** - use `sum()` in queries to aggregate
   across instances
3. **No shared state needed** - each Watchdog instance is independent

Watchdog is almost entirely stateless, so horizontal scaling should be trivial
as long as you have the necessary infrastructure and, well, the patience.
Example with 3 instances:

```promql
# Total pageviews across all instances
sum(rate(web_pageviews_total[5m]))

# Per-instance breakdown
sum by (instance) (rate(web_pageviews_total[5m]))
```

## Alternatives to Prometheus

### VictoriaMetrics

VictoriaMetrics is a fast, cost-effective monitoring solution and time-series
database that is 100% compatible with Prometheus exposition format. Watchdog's
`/metrics` endpoint can be scraped directly by VictoriaMetrics without requiring
Prometheus.

#### Direct Scraping (Recommended)

VictoriaMetrics single-node mode can scrape Watchdog directly using standard
Prometheus scrape configuration:

**Configuration file (`/etc/victoriametrics/scrape.yml`):**

```yaml
scrape_configs:
  - job_name: "watchdog"
    static_configs:
      - targets: ["localhost:8080"]
    scrape_interval: 15s
    metrics_path: /metrics
```

**Run VictoriaMetrics:**

```bash
victoria-metrics -promscrape.config=/etc/victoriametrics/scrape.yml
```

**NixOS configuration:**

```nix
{
  services.victoriametrics = {
    enable = true;
    listenAddress = ":8428";
    retentionPeriod = "12month";

    # Define scrape configs directly. 'prometheusConfig' is the configuration for
    # Prometheus-style metrics endpoints, which Watchdog exports.
    prometheusConfig = {
      scrape_configs = [
        {
          job_name = "watchdog";
          scrape_interval = "15s";
          static_configs = [{
            targets = [ "localhost:8080" ]; # replace the port
          }];
        }
      ];
    };
  };
}
```

#### Using `vmagent`

Alternatively, for distributed setups or when you need more advanced features
like relabeling, you may use `vmagent`:

```nix
{
  services.vmagent = {
    enable = true;
    remoteWriteUrl = "http://localhost:8428/api/v1/write";

    prometheusConfig = {
      scrape_configs = [
        {
          job_name = "watchdog";
          static_configs = [{
            targets = [ "localhost:8080" ];
          }];
        }
      ];
    };
  };

  services.victoriametrics = {
    enable = true;
    listenAddress = ":8428";
  };
}
```

#### Prometheus Remote Write

If you are migrating from Prometheus, or if you need PromQL compatibility, or if
you just really like using Prometheus for some inexplicable reason you may keep
Prometheus but use VictoriaMetrics to remote-write.

```nix
{
  services.prometheus = {
    enable = true;
    port = 9090;

    scrapeConfigs = [
      {
        job_name = "watchdog";
        static_configs = [{
          targets = [ "localhost:8080" ];
        }];
      }
    ];

    remoteWrite = [{
      url = "http://localhost:8428/api/v1/write";
    }];
  };

  services.victoriametrics = {
    enable = true;
    listenAddress = ":8428";
  };
}
```

### Grafana Agent

Lightweight alternative that scrapes and forwards to Grafana Cloud or local
Prometheus:

```bash
# Systemd setup for Grafana Agent
sudo systemctl enable --now grafana-agent
```

```yaml
# /etc/grafana-agent.yaml
metrics:
  wal_directory: /var/lib/grafana-agent
  configs:
    - name: watchdog
      scrape_configs:
        - job_name: watchdog
          static_configs:
            - targets: ["localhost:8080"]
      remote_write:
        - url: http://localhost:9090/api/v1/write
```

## Monitoring the Monitoring

Monitor your scraper:

```promql
# Scrape success rate
up{job="watchdog"}

# Scrape duration
scrape_duration_seconds{job="watchdog"}

# Time since last scrape
time() - timestamp(up{job="watchdog"})
```

For VictoriaMetrics, you can also monitor ingestion stats:

```bash
# VM internal metrics
curl http://localhost:8428/metrics | grep vm_rows_inserted_total
```
