use chrono::{DateTime, Duration, TimeZone, Utc};
use scylla::frame::value::Timestamp;

pub(crate) fn to_timestamp(date_time: &DateTime<Utc>) -> Timestamp {
    Timestamp(Duration::milliseconds(date_time.timestamp_millis()))
}

pub(crate) fn from_timestamp(Timestamp(timestamp): &Timestamp) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(timestamp.num_milliseconds())
        .unwrap()
}
