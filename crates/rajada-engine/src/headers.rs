//! Parsing of response headers that come from untrusted servers.

use std::time::Duration;

/// `Content-Range: bytes a-b/total` → `(a, b_inclusive, Some(total))`.
/// `total` is `None` for `*`. Returns `None` for anything malformed.
pub fn content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (a, b) = range.split_once('-')?;
    let (a, b) = (a.trim().parse::<u64>().ok()?, b.trim().parse::<u64>().ok()?);
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse::<u64>().ok()?),
    };
    if b < a || total.is_some_and(|t| b >= t) {
        return None;
    }
    Some((a, b, total))
}

/// `Retry-After` in delta-seconds. HTTP-date values are not parsed yet and
/// fall back to the caller's default.
pub fn retry_after(v: &str) -> Option<Duration> {
    v.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Extracts the file name from `Content-Disposition`, preferring the
/// RFC 6266/5987 `filename*=UTF-8''...` form.
pub fn content_disposition_filename(v: &str) -> Option<String> {
    let mut plain = None;
    for part in v.split(';').map(str::trim) {
        let Some((k, val)) = part.split_once('=') else {
            continue;
        };
        match k.trim().to_ascii_lowercase().as_str() {
            "filename*" => {
                let val = val.trim();
                let encoded = val
                    .strip_prefix("UTF-8''")
                    .or_else(|| val.strip_prefix("utf-8''"))?;
                return percent_decode(encoded);
            }
            "filename" => plain = Some(val.trim().trim_matches('"').to_owned()),
            _ => {}
        }
    }
    plain
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Makes a server-provided name safe: no path separators, no control
/// characters, no leading dots, bounded length. Never returns an empty name.
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    let mut out = String::new();
    for c in trimmed.chars() {
        if out.len() + c.len_utf8() > 200 {
            break;
        }
        out.push(c);
    }
    if out.is_empty() {
        "download".to_owned()
    } else {
        out
    }
}

/// Last non-empty path segment of a URL, percent-decoded.
pub fn filename_from_url(url: &reqwest::Url) -> Option<String> {
    let seg = url.path_segments()?.rev().find(|s| !s.is_empty())?;
    percent_decode(seg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parses_content_range() {
        assert_eq!(content_range("bytes 0-99/1000"), Some((0, 99, Some(1000))));
        assert_eq!(content_range("bytes 5-5/*"), Some((5, 5, None)));
        assert_eq!(content_range("bytes 9-3/1000"), None);
        assert_eq!(content_range("bytes 0-1000/1000"), None);
        assert_eq!(content_range("items 0-1/2"), None);
    }

    #[test]
    fn parses_disposition() {
        let v = "attachment; filename=\"a b.iso\"; filename*=UTF-8''caf%C3%A9.iso";
        assert_eq!(content_disposition_filename(v).as_deref(), Some("café.iso"));
        assert_eq!(
            content_disposition_filename("attachment; filename=\"x.zip\"").as_deref(),
            Some("x.zip")
        );
        assert_eq!(content_disposition_filename("inline"), None);
    }

    #[test]
    fn sanitizes() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_filename("a\nb"), "a_b");
        assert_eq!(sanitize_filename("..."), "download");
        assert_eq!(sanitize_filename(""), "download");
    }

    proptest! {
        #[test]
        fn sanitized_names_are_always_safe(s in ".*") {
            let n = sanitize_filename(&s);
            prop_assert!(!n.is_empty() && n.len() <= 200);
            prop_assert!(!n.contains('/') && !n.contains('\\') && !n.starts_with('.'));
            prop_assert!(!n.chars().any(char::is_control));
        }

        #[test]
        fn content_range_never_panics(s in ".*") {
            let _ = content_range(&s);
            let _ = content_disposition_filename(&s);
            let _ = retry_after(&s);
        }
    }
}
