use std::time::Duration;

pub const MAX_EVENT_SIZE: usize = 4 * 1024;
pub const MAX_EVENT_NAME_LEN: usize = 120;
pub const MAX_PATH_LEN: usize = 2048;
pub const MAX_PROPERTY_KEY_LEN: usize = 64;
pub const MAX_PROPERTY_VALUE_LEN: usize = 200;
pub const MAX_REFERRER_LEN: usize = 2048;
pub const MAX_ENGAGEMENT_SECONDS: f64 = 86_400.0;
pub const MAX_WIDTH: u16 = 10_000;

pub const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(10);
pub const HTTP_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
pub const UNIQUES_UPDATE_PERIOD: Duration = Duration::from_secs(10);
pub const MAX_METRICS_RESPONSE_SIZE: usize = 10 * 1024 * 1024;
