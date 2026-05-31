use std::time::Duration;

/// Maximum accepted JSON event payload size in bytes.
pub const MAX_EVENT_SIZE: usize = 4 * 1024;
/// Maximum event name length before validation rejects the payload.
pub const MAX_EVENT_NAME_LEN: usize = 120;
/// Maximum path length accepted from incoming events.
pub const MAX_PATH_LEN: usize = 2048;
/// Maximum custom property key length before it is collapsed to `other`.
pub const MAX_PROPERTY_KEY_LEN: usize = 64;
/// Maximum custom property value length before it is collapsed to `other`.
pub const MAX_PROPERTY_VALUE_LEN: usize = 200;
/// Maximum raw referrer length accepted from incoming events.
pub const MAX_REFERRER_LEN: usize = 2048;
/// Maximum engagement duration accepted for a single event.
pub const MAX_ENGAGEMENT_SECONDS: f64 = 86_400.0;
/// Maximum viewport width accepted from incoming events.
pub const MAX_WIDTH: u16 = 10_000;

/// HTTP read timeout used by packaged service integrations.
pub const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// HTTP write timeout used by packaged service integrations.
pub const HTTP_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum time allowed for background tasks to stop during shutdown.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
/// Interval for refreshing the exported unique visitor gauge.
pub const UNIQUES_UPDATE_PERIOD: Duration = Duration::from_secs(10);
/// Maximum Prometheus response size returned by the metrics endpoint.
pub const MAX_METRICS_RESPONSE_SIZE: usize = 10 * 1024 * 1024;
