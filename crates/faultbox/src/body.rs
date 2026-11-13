//! Deterministic, seekable file content.

use sha2::{Digest, Sha256};

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Fills `buf` with the content found at `offset` of the file `seed`.
pub fn fill(seed: u64, offset: u64, buf: &mut [u8]) {
    for (i, b) in buf.iter_mut().enumerate() {
        let pos = offset + i as u64;
        let word = splitmix64(seed ^ (pos / 8).wrapping_mul(0xA24B_AED4_963E_E407));
        *b = word.to_le_bytes()[(pos % 8) as usize];
    }
}

/// Content of `[start, end)` of the file.
pub fn range(seed: u64, start: u64, end: u64) -> Vec<u8> {
    let mut v = vec![0; end.saturating_sub(start) as usize];
    fill(seed, start, &mut v);
    v
}

/// Hex SHA-256 of the whole file, so tests know the expected hash.
pub fn expected_sha256(seed: u64, size: u64) -> String {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut off = 0;
    while off < size {
        let n = (size - off).min(buf.len() as u64) as usize;
        fill(seed, off, &mut buf[..n]);
        h.update(&buf[..n]);
        off += n as u64;
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seekable_and_seed_dependent() {
        let whole = range(1, 0, 100);
        assert_eq!(range(1, 37, 61), whole[37..61]);
        assert_ne!(range(2, 0, 100), whole);
    }
}
