//! RFC 3339 UTC strings on the wire

use chrono::{DateTime, SecondsFormat, Utc};

pub fn now() -> DateTime<Utc> {
    Utc::now()
}

pub fn to_rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn now_rfc3339() -> String {
    to_rfc3339(now())
}
