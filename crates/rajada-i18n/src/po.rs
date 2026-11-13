//! Minimal parser for gettext `.po` files (single-line and continued strings).

use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq)]
pub struct PoError {
    pub line: usize,
    pub message: &'static str,
}

fn unquote(s: &str, line: usize) -> Result<String, PoError> {
    let inner = s
        .trim()
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or(PoError {
            line,
            message: "expected quoted string",
        })?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            _ => {
                return Err(PoError {
                    line,
                    message: "bad escape",
                });
            }
        }
    }
    Ok(out)
}

enum Field {
    None,
    Id,
    Str,
}

/// Parses `.po` text into msgid → msgstr. The header (empty msgid) and
/// untranslated entries (empty msgstr) are skipped.
pub fn parse(text: &str) -> Result<HashMap<String, String>, PoError> {
    let mut map = HashMap::new();
    let (mut id, mut s) = (None::<String>, None::<String>);
    let mut field = Field::None;
    let mut flush = |id: &mut Option<String>, s: &mut Option<String>| {
        if let (Some(i), Some(v)) = (id.take(), s.take())
            && !i.is_empty()
            && !v.is_empty()
        {
            map.insert(i, v);
        }
    };
    for (n, raw) in text.lines().enumerate() {
        let line = n + 1;
        let l = raw.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some(rest) = l.strip_prefix("msgid ") {
            flush(&mut id, &mut s);
            id = Some(unquote(rest, line)?);
            field = Field::Id;
        } else if let Some(rest) = l.strip_prefix("msgstr ") {
            s = Some(unquote(rest, line)?);
            field = Field::Str;
        } else if l.starts_with('"') {
            let part = unquote(l, line)?;
            let target = match field {
                Field::Id => id.as_mut(),
                Field::Str => s.as_mut(),
                Field::None => None,
            };
            target
                .ok_or(PoError {
                    line,
                    message: "continuation without field",
                })?
                .push_str(&part);
        } else {
            return Err(PoError {
                line,
                message: "unknown directive",
            });
        }
    }
    flush(&mut id, &mut s);
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries_and_continuations() {
        let m = parse("msgid \"\"\nmsgstr \"h\"\n\nmsgid \"a\"\nmsgstr \"x\"\n\"y\\n\"\n").unwrap();
        assert_eq!(m.get("a").map(String::as_str), Some("xy\n"));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse("foo").unwrap_err().line, 1);
    }
}
