//! Display formatting for timestamps in the configured zone.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

/// Format a timestamp, or `None` for the epoch placeholder.
pub fn local(time: DateTime<Utc>, timezone: Tz) -> Option<String> {
    if time == DateTime::UNIX_EPOCH {
        return None;
    }

    Some(
        time.with_timezone(&timezone)
            .format("%b %d, %I:%M %p")
            .to_string(),
    )
}
