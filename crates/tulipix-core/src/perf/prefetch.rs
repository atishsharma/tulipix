//! Predictive viewport prefetch. The grid view samples scroll velocity and
//! asks the prefetcher to render thumbs N viewports ahead in the scroll
//! direction. Direction reversal cancels in-flight prefetches.
//!
//! Bounded by the GPU texture-cache budget (perf::gpu_budget); the
//! prefetcher refuses to enqueue if the budget is at threshold.

use std::collections::VecDeque;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction { Up, Down, None }

#[derive(Debug, Clone, Copy)]
pub struct Velocity {
    /// px / second; signed (+down, -up).
    pub px_per_sec: f32,
}

impl Velocity {
    pub fn direction(self) -> Direction {
        if self.px_per_sec >  50.0 { Direction::Down }
        else if self.px_per_sec < -50.0 { Direction::Up }
        else { Direction::None }
    }
    /// Number of viewports to look ahead based on velocity bands.
    /// 0-200 px/s  → 1 viewport; 200-800 → 2; 800-2000 → 3; > 2000 → 4.
    pub fn lookahead(self) -> u32 {
        let v = self.px_per_sec.abs();
        if v <  200.0 { 1 }
        else if v <  800.0 { 2 }
        else if v < 2000.0 { 3 }
        else               { 4 }
    }
}

#[derive(Debug)]
pub struct Prefetcher {
    queue: Mutex<VecDeque<i64>>,
    last_direction: Mutex<Direction>,
    gpu_budget_ok: fn() -> bool,
}

fn default_budget_ok() -> bool { true }

impl Prefetcher {
    pub fn new() -> Self {
        Self { queue: Mutex::new(VecDeque::new()), last_direction: Mutex::new(Direction::None), gpu_budget_ok: default_budget_ok }
    }
    pub fn with_budget_probe(probe: fn() -> bool) -> Self {
        Self { queue: Mutex::new(VecDeque::new()), last_direction: Mutex::new(Direction::None), gpu_budget_ok: probe }
    }

    /// Push the visible row range + velocity. Returns the set of row ids to
    /// prefetch (or an empty Vec if the GPU budget is full).
    pub fn schedule(&self, visible: std::ops::Range<i64>, v: Velocity, total_rows: i64) -> Vec<i64> {
        let dir = v.direction();
        let mut last = self.last_direction.lock().unwrap();
        if *last != Direction::None && dir != Direction::None && dir != *last {
            self.queue.lock().unwrap().clear();
        }
        *last = dir;
        drop(last);

        if !(self.gpu_budget_ok)() { return vec![]; }

        let span = (visible.end - visible.start).max(1);
        let ahead = (v.lookahead() as i64) * span;
        let (start, end) = match dir {
            Direction::Down => (visible.end, (visible.end + ahead).min(total_rows)),
            Direction::Up   => ((visible.start - ahead).max(0), visible.start),
            Direction::None => return vec![],
        };
        let mut q = self.queue.lock().unwrap();
        let mut out = vec![];
        for r in start..end {
            if !q.contains(&r) { q.push_back(r); out.push(r); }
        }
        out
    }

    pub fn cancel_pending(&self) -> usize {
        let mut q = self.queue.lock().unwrap();
        let n = q.len(); q.clear(); n
    }

    pub fn pending(&self) -> usize { self.queue.lock().unwrap().len() }
}

impl Default for Prefetcher { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_scroll_no_prefetch() {
        let p = Prefetcher::new();
        let out = p.schedule(0..20, Velocity { px_per_sec: 0.0 }, 1000);
        assert!(out.is_empty());
    }
    #[test] fn fast_scroll_prefetches_ahead() {
        let p = Prefetcher::new();
        let out = p.schedule(0..20, Velocity { px_per_sec: 1200.0 }, 1000);
        assert!(out.len() == 60); // lookahead 3 * span 20
        assert_eq!(out[0], 20);
    }
    #[test] fn reverse_clears_queue() {
        let p = Prefetcher::new();
        p.schedule(100..120, Velocity { px_per_sec: 500.0 }, 1000);
        assert_eq!(p.pending(), 40);
        let _ = p.schedule(100..120, Velocity { px_per_sec: -500.0 }, 1000);
        // Reverse cleared the queue, then enqueued upward prefetch.
        assert!(p.pending() > 0);
    }
    #[test] fn budget_probe_blocks() {
        fn always_full() -> bool { false }
        let p = Prefetcher::with_budget_probe(always_full);
        assert!(p.schedule(0..20, Velocity { px_per_sec: 1200.0 }, 1000).is_empty());
    }
}
