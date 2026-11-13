//! Locale-aware formatting of sizes, rates and durations.

use crate::Locale;

fn decimal(value: f64, locale: Locale) -> String {
    let s = format!("{value:.1}");
    match locale {
        Locale::PtBr => s.replace('.', ","),
        Locale::En => s,
    }
}

/// Binary units: "15,8 MiB" (pt-BR) / "15.8 MiB" (en). Bytes stay integers.
pub fn bytes(n: u64, locale: Locale) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{} {}", decimal(v, locale), UNITS[i])
}

pub fn rate(bytes_per_sec: u64, locale: Locale) -> String {
    format!("{}/s", bytes(bytes_per_sec, locale))
}

/// "45 s", "2 min 5 s", "1 h 3 min".
pub fn duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    match (h, m) {
        (0, 0) => format!("{s} s"),
        (0, _) if s == 0 => format!("{m} min"),
        (0, _) => format!("{m} min {s} s"),
        _ => format!("{h} h {m} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_per_locale() {
        assert_eq!(bytes(512, Locale::PtBr), "512 B");
        assert_eq!(rate(16_567_500, Locale::PtBr), "15,8 MiB/s");
        assert_eq!(rate(16_567_500, Locale::En), "15.8 MiB/s");
        assert_eq!(bytes(38 * 1024, Locale::En), "38.0 KiB");
        assert_eq!(duration(30), "30 s");
        assert_eq!(duration(125), "2 min 5 s");
        assert_eq!(duration(3780), "1 h 3 min");
    }
}
