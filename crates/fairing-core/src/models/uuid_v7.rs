use anyhow::Result;
use std::time::SystemTime;
use uuid::Uuid;

/// UUIDv7 as per
/// https://datatracker.ietf.org/doc/html/draft-peabody-dispatch-new-uuid-format#section-5.2
pub fn uuid_v7() -> Result<Uuid> {
    let timestamp_ns = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let timestamp_ms = timestamp_ns / 1_000_000;
    // make sure the timestamp is not larger than 48 bits.
    let timestamp_ms = timestamp_ms & 0xffff_ffff_ffff;

    let mut random = [0u8; 10];
    getrandom::getrandom(&mut random)?;

    // 12 bits of random data.
    let random_a = {
        let mut b = [0u8; 2];
        b.copy_from_slice(&random[..2]);
        (u16::from_le_bytes(b) & 0x0fff) as u128
    };

    // 62 bits of random data.
    let random_b = {
        let mut b = [0u8; 8];
        b.copy_from_slice(&random[2..]);
        (u64::from_le_bytes(b) & 0x3fff_ffff_ffff_ffff) as u128
    };

    let uuid = (timestamp_ms << 80) | (0x7 << 76) | (random_a << 64) | (0b01 << 62) | random_b;

    Ok(uuid::Uuid::from_u128(uuid))
}
