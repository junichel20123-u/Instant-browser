//! Adaptive connection controller (spec §7.6).
//!
//! Pure logic, fed once per measurement window with the bytes received from
//! the socket. It ramps up in batches (+1, +2, +4…) while the last addition
//! paid off by more than 10% and latency did not jump, then settles on the
//! last count that helped. 429/503 halve the count. A fixed mode disables it.

use std::time::Duration;

/// Minimum relative gain for an addition to count as useful.
const MIN_GAIN: f64 = 0.10;
/// Latency may grow at most this much over the baseline while ramping...
const MAX_RTT_GROWTH: f64 = 1.5;
/// ...and only growth above this absolute amount counts (loopback and LAN
/// baselines of ~1 ms would otherwise trip on scheduler noise).
const MIN_RTT_JUMP: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Let the controller decide, up to `max` connections.
    Auto { max: usize },
    /// Exactly `n` connections (`--connections fixed:N`).
    Fixed(usize),
}

/// A change of the connection count, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decision {
    pub from: usize,
    pub to: usize,
    /// Gain of the last measured step, in percent (negative = loss).
    pub gain_pct: i32,
    /// True when the controller stopped ramping with this decision.
    pub settled: bool,
    /// False for the first step, taken before any comparison was possible.
    pub measured: bool,
    /// Measured rate (bytes/s) of the connection count chosen (`to`).
    pub rate: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Measuring the rate at `target`; `prev` is the (count, rate) before.
    Ramp {
        prev: Option<(usize, f64)>,
        step: usize,
    },
    Hold,
}

#[derive(Debug, Clone)]
pub struct Controller {
    mode: Mode,
    target: usize,
    phase: Phase,
    /// Windows to ignore after a change (connections warming up).
    skip: u32,
    rtt_baseline: Option<Duration>,
    rtt_samples: Vec<Duration>,
    /// Exponential moving average of the rate (bytes/s), for display.
    ewma: f64,
    best_rate: f64,
}

impl Controller {
    pub fn new(mode: Mode) -> Controller {
        let target = match mode {
            Mode::Auto { .. } => 1,
            Mode::Fixed(n) => n.max(1),
        };
        let phase = match mode {
            Mode::Auto { .. } => Phase::Ramp {
                prev: None,
                step: 1,
            },
            Mode::Fixed(_) => Phase::Hold,
        };
        Controller {
            mode,
            target,
            phase,
            // The first window mixes connection setup with transfer.
            skip: 1,
            rtt_baseline: None,
            rtt_samples: Vec::new(),
            ewma: 0.0,
            best_rate: 0.0,
        }
    }

    pub fn target(&self) -> usize {
        self.target
    }

    pub fn best_rate(&self) -> f64 {
        self.best_rate
    }

    #[cfg(test)]
    pub fn is_settled(&self) -> bool {
        self.phase == Phase::Hold
    }

    fn max(&self) -> usize {
        match self.mode {
            Mode::Auto { max } => max.max(1),
            Mode::Fixed(n) => n.max(1),
        }
    }

    /// Time to first byte of a request: a cheap proxy for RTT.
    pub fn rtt_sample(&mut self, d: Duration) {
        self.rtt_samples.push(d);
    }

    fn rtt_ok(&mut self) -> bool {
        let mut s = std::mem::take(&mut self.rtt_samples);
        if s.is_empty() {
            return true;
        }
        s.sort_unstable();
        let p50 = s[s.len() / 2];
        match self.rtt_baseline {
            None => {
                self.rtt_baseline = Some(p50);
                true
            }
            Some(base) => {
                p50.as_secs_f64() <= base.as_secs_f64() * MAX_RTT_GROWTH
                    || p50.saturating_sub(base) < MIN_RTT_JUMP
            }
        }
    }

    /// Feeds one measurement window. Returns a decision when the target changes
    /// or when the controller settles.
    pub fn on_window(&mut self, bytes: u64, window: Duration) -> Option<Decision> {
        let rate = bytes as f64 / window.as_secs_f64().max(1e-3);
        self.ewma = if self.ewma == 0.0 {
            rate
        } else {
            0.5 * rate + 0.5 * self.ewma
        };
        self.best_rate = self.best_rate.max(self.ewma);
        if self.skip > 0 {
            self.skip -= 1;
            return None;
        }
        let Phase::Ramp { prev, step } = self.phase else {
            return None;
        };
        let rtt_ok = self.rtt_ok();
        let gain = prev.map(|(_, r)| if r > 0.0 { rate / r - 1.0 } else { 1.0 });
        let helped = gain.is_none_or(|g| g > MIN_GAIN) && rtt_ok;
        let from = self.target;
        let gain_pct = gain.map_or(0, |g| (g * 100.0).round() as i32);
        if !helped {
            // The last addition did not pay off: go back to what helped.
            let (back, back_rate) = prev.unwrap_or((from, rate));
            self.target = back;
            self.phase = Phase::Hold;
            return Some(Decision {
                from,
                to: back,
                gain_pct,
                settled: true,
                measured: true,
                rate: back_rate,
            });
        }
        if from >= self.max() {
            self.phase = Phase::Hold;
            return Some(Decision {
                from,
                to: from,
                gain_pct,
                settled: true,
                measured: prev.is_some(),
                rate,
            });
        }
        let to = (from + step).min(self.max());
        self.target = to;
        self.phase = Phase::Ramp {
            prev: Some((from, rate)),
            step: step * 2,
        };
        Some(Decision {
            from,
            to,
            gain_pct,
            settled: false,
            measured: prev.is_some(),
            rate,
        })
    }

    /// The server pushed back (429/503 or a burst of resets): halve.
    pub fn on_throttle(&mut self) -> Option<Decision> {
        let from = self.target;
        let to = (from / 2).max(1);
        self.target = to;
        self.phase = Phase::Hold;
        (to != from).then_some(Decision {
            from,
            to,
            gain_pct: 0,
            settled: true,
            measured: true,
            rate: self.ewma,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: Duration = Duration::from_millis(500);
    const MIB: f64 = 1048576.0;

    /// Simulates a server; returns (final target, windows until settled).
    fn simulate(rate_for: impl Fn(usize, u32) -> f64, windows: u32) -> (usize, Option<u32>) {
        let mut c = Controller::new(Mode::Auto { max: 8 });
        let mut settled_at = None;
        for w in 0..windows {
            let bytes = (rate_for(c.target(), w) * W.as_secs_f64()) as u64;
            c.on_window(bytes, W);
            if c.is_settled() && settled_at.is_none() {
                settled_at = Some(w + 1);
            }
        }
        (c.target(), settled_at)
    }

    #[test]
    fn per_connection_limit_reaches_eight_within_three_seconds() {
        let (n, settled) = simulate(|n, _| (n as f64 * 2.0 * MIB).min(16.0 * MIB), 20);
        assert_eq!(n, 8);
        // Reaching 8 connections takes 1+2+2+2 windows = 3.5 s at most,
        // but the target already hits 8 after the third step (≤ 3 s).
        assert!(settled.unwrap() <= 8, "{settled:?}");
    }

    #[test]
    fn target_reaches_eight_by_three_seconds() {
        let mut c = Controller::new(Mode::Auto { max: 8 });
        let mut reached = None;
        for w in 0..10u32 {
            let rate = (c.target() as f64 * 2.0 * MIB).min(16.0 * MIB);
            c.on_window((rate * 0.5) as u64, W);
            if c.target() == 8 && reached.is_none() {
                reached = Some((w + 1) as f64 * 0.5);
            }
        }
        assert!(reached.unwrap() <= 3.0, "{reached:?}");
    }

    #[test]
    fn no_per_connection_limit_stays_at_one_or_two() {
        let (n, settled) = simulate(|_, _| 50.0 * MIB, 20);
        assert!(n <= 2, "{n}");
        assert!(settled.is_some());
    }

    #[test]
    fn global_cap_stops_ramp_where_gains_end() {
        // 2 MiB/s per connection but only 6 MiB/s in total.
        let (n, _) = simulate(|n, _| (n as f64 * 2.0 * MIB).min(6.0 * MIB), 20);
        assert!((2..=4).contains(&n), "{n}");
    }

    #[test]
    fn noise_does_not_cause_oscillation() {
        let noisy = |n: usize, w: u32| {
            let jitter = if w.is_multiple_of(2) { 1.05 } else { 0.95 };
            (n as f64 * 2.0 * MIB).min(16.0 * MIB) * jitter
        };
        let mut c = Controller::new(Mode::Auto { max: 8 });
        let mut changes = 0;
        for w in 0..40 {
            let bytes = (noisy(c.target(), w) * 0.5) as u64;
            if c.on_window(bytes, W).is_some_and(|d| d.from != d.to) {
                changes += 1;
            }
        }
        assert_eq!(c.target(), 8);
        assert!(changes <= 3, "{changes} changes");
    }

    #[test]
    fn throttle_halves() {
        let mut c = Controller::new(Mode::Fixed(8));
        let d = c.on_throttle().unwrap();
        assert_eq!((d.from, d.to), (8, 4));
        c.on_throttle();
        c.on_throttle();
        assert_eq!(c.target(), 1);
        assert!(c.on_throttle().is_none());
    }

    #[test]
    fn rtt_jump_stops_the_ramp() {
        let mut c = Controller::new(Mode::Auto { max: 8 });
        c.on_window(0, W); // first window skipped (warm-up)
        c.rtt_sample(Duration::from_millis(10));
        c.on_window((2.0 * MIB) as u64, W); // 1 -> 2
        c.rtt_sample(Duration::from_millis(100));
        let d = c.on_window((4.0 * MIB) as u64, W).unwrap();
        assert!(d.settled);
        assert_eq!(c.target(), 1);
    }

    #[test]
    fn fixed_mode_never_changes_on_its_own() {
        let mut c = Controller::new(Mode::Fixed(3));
        for _ in 0..10 {
            assert!(c.on_window(1000, W).is_none());
        }
        assert_eq!(c.target(), 3);
    }
}
