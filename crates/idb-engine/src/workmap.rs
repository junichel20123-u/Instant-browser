//! Dynamic work map: byte ranges ("leases") handed to connections, with
//! work stealing.
//!
//! A lease covers `[start, end)`. Its worker *claims* bytes as they arrive
//! from the socket (`received`) and reports them as `written` once they are
//! on disk. Stealing cuts a lease at a point past `received`, so bytes that
//! were received but are still buffered never fall beyond the new end.
//! Every lease has its own small lock; there is no global lock.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use idb_store::SegmentRecord;

#[derive(Debug)]
struct LeaseState {
    end: u64,
    /// Bytes from `start` accepted from the network (written or buffered).
    received: u64,
    /// Bytes from `start` written to the file.
    written: u64,
    last_byte: Instant,
}

/// One byte range and its progress. Shared between the coordinator and
/// at most one worker.
#[derive(Debug)]
pub struct Lease {
    pub start: u64,
    state: Mutex<LeaseState>,
}

impl Lease {
    fn new(start: u64, end: u64, written: u64) -> Arc<Lease> {
        Arc::new(Lease {
            start,
            state: Mutex::new(LeaseState {
                end,
                received: written,
                written,
                last_byte: Instant::now(),
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, LeaseState> {
        // A poisoned lock only means a worker panicked mid-update; the
        // counters are still valid integers, so keep going.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn end(&self) -> u64 {
        self.lock().end
    }

    /// Absolute position of the next byte to write.
    pub fn write_pos(&self) -> u64 {
        self.start + self.lock().written
    }

    pub fn written(&self) -> u64 {
        self.lock().written
    }

    pub fn remaining(&self) -> u64 {
        let s = self.lock();
        (s.end - self.start).saturating_sub(s.written)
    }

    /// Bytes not yet received from the network (what a thief could take).
    pub fn unreceived(&self) -> u64 {
        let s = self.lock();
        s.end.saturating_sub(self.start + s.received)
    }

    pub fn is_finished(&self) -> bool {
        self.remaining() == 0
    }

    pub fn idle_for(&self) -> std::time::Duration {
        self.lock().last_byte.elapsed()
    }

    /// Accepts up to `n` bytes arriving at absolute position `pos`.
    /// Returns how many fit before the (possibly shrunk) end; 0 means stop.
    pub fn claim(&self, pos: u64, n: u64) -> u64 {
        let mut s = self.lock();
        let at = self.start + s.received;
        if pos != at {
            return 0;
        }
        let ok = n.min(s.end.saturating_sub(at));
        s.received += ok;
        if ok > 0 {
            s.last_byte = Instant::now();
        }
        ok
    }

    /// Records `n` more bytes written to disk (in order).
    pub fn wrote(&self, n: u64) {
        let mut s = self.lock();
        s.written = (s.written + n).min(s.received);
    }

    /// Forgets bytes that were received but never written (the worker died).
    fn reset_to_written(&self) {
        let mut s = self.lock();
        s.received = s.written;
        s.last_byte = Instant::now();
    }

    /// Cuts the lease at `received + keep` (rounded), returning the old end.
    /// The cut never goes below what was already received.
    fn cut(&self, keep_fraction_of_rest: Option<u64>) -> Option<(u64, u64)> {
        let mut s = self.lock();
        let from = self.start + s.received;
        let rest = s.end.saturating_sub(from);
        let keep = match keep_fraction_of_rest {
            Some(div) => rest / div,
            None => 0,
        };
        let at = from + keep;
        if at >= s.end {
            return None;
        }
        let old_end = s.end;
        s.end = at;
        Some((at, old_end))
    }
}

/// The set of leases for one download; owned by the coordinator.
pub struct WorkMap {
    leases: Vec<(Arc<Lease>, bool)>,
    /// A lease is split only if each half gets at least this many bytes.
    min_split: u64,
}

impl WorkMap {
    pub fn new(total: u64, min_split: u64) -> WorkMap {
        WorkMap {
            leases: vec![(Lease::new(0, total, 0), false)],
            min_split: min_split.max(1),
        }
    }

    /// Rebuilds the map from journal records (resume).
    pub fn from_records(records: &[SegmentRecord], min_split: u64) -> WorkMap {
        WorkMap {
            leases: records
                .iter()
                .map(|r| (Lease::new(r.start, r.end, r.done), false))
                .collect(),
            min_split: min_split.max(1),
        }
    }

    /// Splits `[0,total)` into an even initial plan (fixed connection count).
    pub fn with_plan(segs: &[crate::segments::Segment], min_split: u64) -> WorkMap {
        WorkMap {
            leases: segs
                .iter()
                .map(|s| (Lease::new(s.start, s.end, 0), false))
                .collect(),
            min_split: min_split.max(1),
        }
    }

    /// Gives an idle connection something to do: a free lease, or the back
    /// half of the busy lease with the most bytes left (work stealing).
    pub fn assign(&mut self) -> Option<Arc<Lease>> {
        if let Some(entry) = self
            .leases
            .iter_mut()
            .find(|(l, owned)| !*owned && !l.is_finished())
        {
            entry.1 = true;
            return Some(entry.0.clone());
        }
        self.steal()
    }

    fn steal(&mut self) -> Option<Arc<Lease>> {
        let donor = self
            .leases
            .iter()
            .filter(|(_, owned)| *owned)
            .map(|(l, _)| l)
            .max_by_key(|l| l.unreceived())?
            .clone();
        if donor.unreceived() < 2 * self.min_split {
            return None;
        }
        let (at, old_end) = donor.cut(Some(2))?;
        let lease = Lease::new(at, old_end, 0);
        self.leases.push((lease.clone(), true));
        Some(lease)
    }

    /// The worker of `lease` stopped (finished, failed or was cancelled).
    pub fn release(&mut self, lease: &Arc<Lease>) {
        if let Some(entry) = self.leases.iter_mut().find(|(l, _)| Arc::ptr_eq(l, lease)) {
            lease.reset_to_written();
            entry.1 = false;
        }
    }

    /// Stops `lease` at what it already received; the rest becomes a free
    /// lease (parking a connection). Returns false if nothing was left.
    pub fn park(&mut self, lease: &Arc<Lease>) -> bool {
        match lease.cut(None) {
            Some((at, old_end)) => {
                self.leases.push((Lease::new(at, old_end, 0), false));
                true
            }
            None => false,
        }
    }

    /// Busy lease with the most bytes still to receive (candidate for parking).
    pub fn largest_unreceived(&self) -> Option<Arc<Lease>> {
        self.leases
            .iter()
            .filter(|(l, owned)| *owned && l.unreceived() > 0)
            .max_by_key(|(l, _)| l.unreceived())
            .map(|(l, _)| l.clone())
    }

    pub fn owned(&self) -> impl Iterator<Item = &Arc<Lease>> {
        self.leases
            .iter()
            .filter(|(_, owned)| *owned)
            .map(|(l, _)| l)
    }

    pub fn is_complete(&self) -> bool {
        self.leases.iter().all(|(l, _)| l.is_finished())
    }

    pub fn written(&self) -> u64 {
        self.leases.iter().map(|(l, _)| l.written()).sum()
    }

    /// Ranges and written bytes, in file order, for the journal.
    /// Take it *before* `fdatasync`: everything it claims is then durable.
    pub fn snapshot(&self) -> Vec<SegmentRecord> {
        let mut v: Vec<SegmentRecord> = self
            .leases
            .iter()
            .map(|(l, _)| {
                let s = l.lock();
                SegmentRecord {
                    start: l.start,
                    end: s.end,
                    done: s.written,
                }
            })
            .collect();
        v.sort_by_key(|r| r.start);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn check(map: &WorkMap, total: u64) -> Result<(), TestCaseError> {
        let snap = map.snapshot();
        prop_assert_eq!(snap.first().map(|r| r.start), Some(0));
        prop_assert_eq!(snap.last().map(|r| r.end), Some(total));
        for w in snap.windows(2) {
            prop_assert_eq!(w[0].end, w[1].start, "gap or overlap: {:?}", snap);
        }
        for (l, _) in &map.leases {
            let s = l.lock();
            prop_assert!(s.written <= s.received);
            prop_assert!(l.start + s.received <= s.end);
        }
        Ok(())
    }

    #[derive(Debug, Clone)]
    enum Op {
        Assign,
        Progress(usize, u64, u64),
        Release(usize),
        Park(usize),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            Just(Op::Assign),
            (0usize..16, 0u64..50_000, 0u64..50_000).prop_map(|(i, a, b)| Op::Progress(i, a, b)),
            (0usize..16).prop_map(Op::Release),
            (0usize..16).prop_map(Op::Park),
        ]
    }

    proptest! {
        #[test]
        fn coverage_holds_under_any_schedule(
            total in 1u64..2_000_000, min_split in 1u64..100_000, ops in prop::collection::vec(op(), 0..200)
        ) {
            let mut map = WorkMap::new(total, min_split);
            let mut active: Vec<Arc<Lease>> = Vec::new();
            for op in ops {
                match op {
                    Op::Assign => if let Some(l) = map.assign() { active.push(l) },
                    Op::Progress(i, recv, write) => if let Some(l) = active.get(i % active.len().max(1)) {
                        let pos = l.start + l.lock().received;
                        let got = l.claim(pos, recv);
                        l.wrote(write.min(got));
                    },
                    Op::Release(i) => if !active.is_empty() {
                        let l = active.remove(i % active.len());
                        map.release(&l);
                    },
                    Op::Park(i) => if let Some(l) = active.get(i % active.len().max(1)) {
                        map.park(l);
                    },
                }
                check(&map, total)?;
            }
            // Finishing everything (with new workers) completes the file.
            for l in active.drain(..) { map.release(&l); }
            while let Some(l) = map.assign() {
                let pos = l.write_pos();
                let n = l.claim(pos, u64::MAX);
                l.wrote(n);
                map.release(&l);
            }
            prop_assert!(map.is_complete());
            prop_assert_eq!(map.written(), total);
            check(&map, total)?;
        }
    }

    #[test]
    fn steal_takes_back_half_and_respects_received() {
        let mut map = WorkMap::new(10_000, 1000);
        let a = map.assign().unwrap();
        assert_eq!(a.claim(0, 2000), 2000); // 2000 received, 0 written
        let b = map.assign().unwrap();
        assert_eq!((b.start, b.end()), (6000, 10_000));
        assert_eq!(a.end(), 6000);
        // Bytes past the new end are refused.
        assert_eq!(a.claim(2000, 9000), 4000);
        // `a` received everything up to its end: the thief splits `b` instead.
        let c = map.assign().unwrap();
        assert_eq!((c.start, c.end()), (8000, 10_000));
        assert_eq!(b.end(), 8000);
        // 2000 left: one more split into 1000 + 1000, then nothing is
        // big enough to split.
        let d = map.assign().unwrap();
        assert_eq!(d.end() - d.start, 1000);
        while map.assign().is_some() {}
        assert!(map.assign().is_none());
    }

    #[test]
    fn park_frees_the_rest() {
        let mut map = WorkMap::new(1000, 1);
        let a = map.assign().unwrap();
        a.claim(0, 300);
        a.wrote(300);
        assert!(map.park(&a));
        assert!(a.is_finished());
        let b = map.assign().unwrap();
        assert_eq!((b.start, b.end()), (300, 1000));
    }
}
