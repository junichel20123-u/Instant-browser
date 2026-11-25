//! Global speed limit (`--limit`): one token bucket shared by every
//! connection of a download.

use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// Paces byte consumption to `rate` bytes/s with a virtual clock: each
/// caller reserves its slot and sleeps until it is due. A small burst
/// allowance avoids penalising the first chunks.
#[derive(Debug)]
pub struct RateLimiter {
    rate: u64,
    next: Mutex<Option<Instant>>,
}

/// How far behind real time the virtual clock may run (burst allowance).
const BURST: Duration = Duration::from_millis(100);

impl RateLimiter {
    pub fn new(bytes_per_sec: u64) -> RateLimiter {
        RateLimiter {
            rate: bytes_per_sec.max(1),
            next: Mutex::new(None),
        }
    }

    pub fn rate(&self) -> u64 {
        self.rate
    }

    /// Waits until `n` more bytes may be consumed.
    pub async fn acquire(&self, n: u64) {
        let cost = Duration::from_secs_f64(n as f64 / self.rate as f64);
        let due = {
            let now = Instant::now();
            let mut next = self
                .next
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let start = next.map_or(now, |t| t.max(now.checked_sub(BURST).unwrap_or(now)));
            let due = start + cost;
            *next = Some(due);
            due
        };
        tokio::time::sleep_until(due).await;
    }
}

/// Parses `2M`, `500k`, `1.5MiB`, `800KB/s`, `1000000` into bytes/s
/// (binary multiples: k = 1024).
pub fn parse_rate(s: &str) -> Option<u64> {
    let t = s.trim().trim_end_matches("/s").trim();
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let value: f64 = num.parse().ok()?;
    let mult: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = value * mult;
    (bytes.is_finite() && bytes >= 1.0).then_some(bytes as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rates() {
        assert_eq!(parse_rate("2M"), Some(2 << 20));
        assert_eq!(parse_rate("500k"), Some(500 * 1024));
        assert_eq!(parse_rate("1.5MiB"), Some(1_572_864));
        assert_eq!(parse_rate("800KB/s"), Some(800 * 1024));
        assert_eq!(parse_rate("1000"), Some(1000));
        assert_eq!(parse_rate("0"), None);
        assert_eq!(parse_rate("fast"), None);
        assert_eq!(parse_rate("3x"), None);
    }

    #[tokio::test(start_paused = true)]
    async fn paces_to_the_rate() {
        let l = RateLimiter::new(1000);
        let t = Instant::now();
        for _ in 0..10 {
            l.acquire(500).await;
        }
        // 5000 bytes at 1000 B/s = 5 s (minus at most the burst allowance).
        let e = t.elapsed();
        assert!(
            e >= Duration::from_millis(4900) && e <= Duration::from_millis(5100),
            "{e:?}"
        );
    }
}
