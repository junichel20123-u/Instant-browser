//! Final verification and atomic publication of a finished `.part` file.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Hex SHA-256 of a file, computed off the async runtime.
pub async fn sha256_file(path: PathBuf) -> std::io::Result<String> {
    tokio::task::spawn_blocking(move || {
        let mut f = std::fs::File::open(path)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    })
    .await
    .map_err(std::io::Error::other)?
}

/// First free name: `name`, `name (1).ext`, `name (2).ext`...
pub fn unique_destination(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (1..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffixes_existing_names() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            unique_destination(d.path(), "a.iso"),
            d.path().join("a.iso")
        );
        std::fs::write(d.path().join("a.iso"), b"").unwrap();
        assert_eq!(
            unique_destination(d.path(), "a.iso"),
            d.path().join("a (1).iso")
        );
        std::fs::write(d.path().join("a (1).iso"), b"").unwrap();
        assert_eq!(
            unique_destination(d.path(), "a.iso"),
            d.path().join("a (2).iso")
        );
    }
}
