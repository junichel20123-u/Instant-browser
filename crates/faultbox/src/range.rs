//! `Range: bytes=...` parsing (single range only).

/// Parses a single byte range against `size`. Returns `[start, end)`,
/// `Ok(None)` for an unparseable header (ignored per RFC 9110) and
/// `Err(())` when it is unsatisfiable (416).
pub fn parse(header: &str, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((a, b)) = spec.split_once('-') else {
        return Ok(None);
    };
    let (a, b) = (a.trim(), b.trim());
    let parsed = match (a.is_empty(), b.is_empty()) {
        (true, false) => b
            .parse::<u64>()
            .ok()
            .map(|n| (size.saturating_sub(n), size)),
        (false, true) => a.parse::<u64>().ok().map(|s| (s, size)),
        (false, false) => match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(s), Ok(e)) if e >= s => Some((s, (e + 1).min(size))),
            _ => None,
        },
        (true, true) => None,
    };
    match parsed {
        None => Ok(None),
        Some((s, e)) if s >= size || s >= e => Err(()),
        Some(r) => Ok(Some(r)),
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn forms() {
        assert_eq!(parse("bytes=0-", 10), Ok(Some((0, 10))));
        assert_eq!(parse("bytes=2-4", 10), Ok(Some((2, 5))));
        assert_eq!(parse("bytes=5-99", 10), Ok(Some((5, 10))));
        assert_eq!(parse("bytes=-3", 10), Ok(Some((7, 10))));
        assert_eq!(parse("bytes=10-", 10), Err(()));
        assert_eq!(parse("bytes=4-2", 10), Ok(None));
        assert_eq!(parse("items=1-2", 10), Ok(None));
    }
}
