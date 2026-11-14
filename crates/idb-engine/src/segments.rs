//! Partitioning a file into byte ranges for parallel connections.

/// A half-open byte range `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub start: u64,
    pub end: u64,
}

impl Segment {
    pub fn len(&self) -> u64 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// Splits `[0, total)` into at most `n` contiguous segments of at least
/// `min_seg` bytes (the last one may be shorter only when `total < min_seg`).
/// Always returns at least one segment for `total > 0`.
pub fn plan(total: u64, n: usize, min_seg: u64) -> Vec<Segment> {
    if total == 0 {
        return Vec::new();
    }
    let by_size = (total / min_seg.max(1)).max(1);
    let count = (n.max(1) as u64).min(by_size);
    let base = total / count;
    let extra = total % count;
    let mut out = Vec::with_capacity(count as usize);
    let mut start = 0;
    for i in 0..count {
        let len = base + u64::from(i < extra);
        out.push(Segment {
            start,
            end: start + len,
        });
        start += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn small_files_get_one_segment() {
        assert_eq!(plan(10, 8, 1 << 20), vec![Segment { start: 0, end: 10 }]);
        assert!(plan(0, 4, 1).is_empty());
        assert_eq!(plan(10, 3, 1).len(), 3);
    }

    proptest! {
        #[test]
        fn covers_file_without_gaps_or_overlap(
            total in 1u64..1 << 40, n in 1usize..32, min_seg in 1u64..1 << 24
        ) {
            let segs = plan(total, n, min_seg);
            prop_assert!(!segs.is_empty() && segs.len() <= n);
            prop_assert_eq!(segs[0].start, 0);
            prop_assert_eq!(segs.last().map(|s| s.end), Some(total));
            for w in segs.windows(2) {
                prop_assert_eq!(w[0].end, w[1].start);
            }
            for s in &segs {
                prop_assert!(!s.is_empty());
                if segs.len() > 1 {
                    prop_assert!(s.len() >= min_seg);
                }
            }
        }
    }
}
