# Grafana Dashboard

We provide a _sample_ Grafana dashboard for Watchdog, with complete support for
multi-host and multi-site deployments as much as possible. It should be noted,
however, that this is designed to be a _reference_ more than anything. Updates
cannot be provided, and it is recommended that you _write your own dashboard_
while using this one as a reference.

Nevertheless, here are some features provided by the sample dashboard at
`watchdog.json` that you may be interested in:

- **Multi-Instance Support**: Filter and aggregate across multiple Watchdog
  instances
- **Multi-Site Support**: Filter by domain for multi-site deployments
- **Real-time Metrics**: Auto-refresh every 30 seconds
- **Traffic Analysis**: Pageviews, unique visitors, device breakdown, geographic
  distribution
- **Top Content**: Top pages, referrers, custom events
- **System Health**: Instance health, cardinality overflow monitoring, request
  rates

To import it, go to "Dashboards" on your Grafana instance then hit "Import".
Upload the JSON file, select your Prometheus data source (assuming you have a
scraper set up) and hit Import.

## Dashboard Variables

The dashboard includes three template variables for flexible filtering:

### Data Source

- **Variable**: `$datasource`
- **Type**: Data source selector
- **Default**: Prometheus
- **Usage**: Select which Prometheus instance to query

### Instance

- **Variable**: `$instance`
- **Type**: Multi-select query variable
- **Default**: All instances
- **Query**: `label_values(web_pageviews_total, instance)`
- **Usage**: Filter by specific Watchdog instances (e.g., `watchdog-1:8080`,
  `watchdog-2:8080`)

### Domain

- **Variable**: `$domain`
- **Type**: Multi-select query variable
- **Default**: All domains
- **Query**: `label_values(web_pageviews_total{instance=~"$instance"}, domain)`
- **Usage**: Filter by specific domains for multi-site analytics

**Example filters:**

- View all sites across all instances: Instance=All, Domain=All
- View single site across all instances: Instance=All, Domain=example.com
- View single instance, all sites: Instance=watchdog-1:8080, Domain=All
- View single site on single instance: Instance=watchdog-1:8080,
  Domain=example.com

## Dashboard Sections

### Overview Row

- **Unique Visitors (Today)**: Current HyperLogLog estimate across selected
  instances/domains
- **Pageviews/min**: Real-time pageview rate
- **Total Pageviews**: Total pageviews in selected time range
- **Cardinality Overflow/min**: Health indicator (should be ~0)
- **Pageviews by Domain**: Time series showing traffic per domain
- **Unique Visitors by Domain**: Time series showing unique visitors per domain

### Traffic Analysis Row

- **Device Breakdown**: Pie chart of mobile/tablet/desktop traffic
- **Top 10 Countries**: Geographic distribution of traffic
- **Top 20 Pages**: Most visited pages with heat map
- **Top 15 Referrers**: Traffic sources (excludes direct traffic)
- **Top 15 Custom Events**: Most triggered custom events

### System Health Row

- **Instance Health**: Uptime status for each Watchdog instance (1=up, 0=down)
- **Cardinality Overflow**: Rate of rejected metrics due to cardinality limits
  (should be near zero)
- **Request Rate by Instance**: Request throughput per instance

## Metrics Reference

All metrics aggregated using `sum()` across selected instances:

```promql
# Total unique visitors with the default daily rotation
sum(web_daily_unique_visitors{instance=~"$instance",domain=~"$domain"})

# Use web_hourly_unique_visitors instead when salt_rotation is hourly.

# Pageview rate
sum(rate(web_pageviews_total{instance=~"$instance",domain=~"$domain"}[$__rate_interval])) * 60

# Top pages
topk(20, sum(increase(web_pageviews_total{instance=~"$instance",domain=~"$domain"}[$__range])) by (path))

# Device breakdown
sum(increase(web_pageviews_total{instance=~"$instance",domain=~"$domain"}[$__range])) by (device)

# Cardinality health
rate(web_path_overflow_total{instance=~"$instance"}[$__rate_interval]) * 60
```

### Modify Time Range

Default: Last 24 hours

To change:

1. Dashboard Settings -> Time Options
2. Set default time range
3. Save dashboard

### Add Alerts

Example alert for cardinality overflow:

1. Edit "Cardinality Overflow" panel
2. Click **Alert** tab
3. Create alert rule:
   - Condition: `WHEN max() OF query(A,5m,now) IS ABOVE 10`
   - Message: "Cardinality limits are being hit - increase
     max_paths/max_sources/max_custom_events"

## Multi-Instance Aggregation

When running multiple Watchdog instances, Prometheus automatically aggregates
metrics. You may use Prometheus' query language (Promql) to create some queries
to visualise data in various ways. Some examples would be:

**Per-instance breakdown:**

```promql
sum(rate(web_pageviews_total[$__rate_interval])) by (instance)
```

**Total across all instances:**

```promql
sum(rate(web_pageviews_total[$__rate_interval]))
```

**Unique visitors (note: HLL counts don't sum directly):**

```promql
# Approximate total - slight overcount due to HLL properties
sum(web_daily_unique_visitors)

# Per-instance (accurate)
web_daily_unique_visitors
```
