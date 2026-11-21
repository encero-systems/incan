//! When a compiler worker retires: the lifecycle policy, apart from the sockets it is applied to.
//!
//! A worker serves one activation at a time. It retires after its idle period without a request; when what it retains
//! stays more than `FOOTPRINT_MARGIN` bytes above the least it has retained after a compile for `FOOTPRINT_STRIKES`
//! compiling activations in a row; at once when its physical footprint after an activation exceeds
//! `FOOTPRINT_CEILING`; when a request of another protocol or generation reaches it; when nothing can reach it any
//! more; and when the process could not be restored after an activation. A contained compiler panic does not retire it,
//! and neither does a rustc handoff whose footprint after the released heap is within those bounds.
//!
//! An editor check or a successful build that publishes compiler records keeps its worker for 15 minutes of idleness,
//! as does such a build that handed rustc to its caller after lowering; the first successful retained-project
//! activation establishes its memory baseline. A light activation (formatting, a
//! source listing, an import collection) is held only to the ceiling. What a worker retains is its live heap when the
//! entry can read it, else its physical footprint, judged against the least it kept after a compile.

use std::time::Instant;

use crate::protocol::{Fault, Reply};

pub const IDLE_SECONDS: i64 = 10;
pub const RETAINED_IDLE_MILLIS: i64 = 900_000;
pub const FOOTPRINT_MARGIN: i64 = 16_777_216;
pub const FOOTPRINT_STRIKES: i64 = 2;
/// A compile of the lowering library through a worker passes 6.5 GB; a dev-loop harness of every subject peaks at
/// 182 MB. One GiB leaves more than five times that peak.
pub const FOOTPRINT_CEILING: i64 = 1_073_741_824;

/// How the compiler entry ended one activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivationOutcome {
    /// The command ran and exits with this status, as its short-lived command would.
    Exited(i64),
    /// The activation could not begin; nothing ran.
    Unavailable(String),
    /// The compiler panicked; the entry contained the panic and restored its own state.
    Panicked,
    /// The process could not be restored after the activation.
    UnsafeCleanup(String),
    /// The compile's footprint was above the ceiling, so the entry wrote its rustc handoff to this path instead.
    HandedOff(String),
}

/// Why a worker retires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retirement {
    Idle,
    /// Nothing can reach the worker any more: its socket path leads elsewhere, or its home is gone.
    Unreachable,
    Footprint,
    Incompatible,
    /// The caller requested one activation, so successful cleanup is followed by process teardown.
    ActivationComplete,
    /// The process state is unsafe for any further request, so no queued request is served either.
    Unsafe,
}

/// The reply to an activation and the retirement it causes, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settled {
    pub reply: Reply,
    pub retirement: Option<Retirement>,
}

/// A worker's limits: its idle period, its margin over the least it has retained, and its footprint ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub idle_millis: i64,
    pub margin: i64,
    pub ceiling: i64,
    pub retained_idle_millis: i64,
}

impl Bounds {
    /// Bounds with the standard retained-work idle period.
    pub fn new(idle_millis: i64, margin: i64, ceiling: i64) -> Self {
        Self {
            idle_millis,
            margin,
            ceiling,
            retained_idle_millis: RETAINED_IDLE_MILLIS,
        }
    }
}

/// The policy state of one worker: idleness, the least it has retained and its run of compiles over that.
#[derive(Clone, Debug)]
pub struct Lifecycle {
    pub bounds: Bounds,
    /// The least physical footprint after a compiling activation; negative before the first one measured.
    pub kept: i64,
    /// Consecutive compiling activations that left more than the margin above the least the worker had retained.
    pub strikes: i64,
    last_activity: Instant,
    last_retained_activity: Option<Instant>,
    retained_baseline_established: bool,
    /// The least bytes in live allocations after a compiling activation; negative before the first one measured.
    pub live_kept: i64,
}

fn elapsed_millis(since: Instant) -> i64 {
    i64::try_from(since.elapsed().as_millis()).unwrap_or(i64::MAX)
}

impl Lifecycle {
    /// Start a worker policy now.
    pub fn start(bounds: Bounds) -> Self {
        Self {
            bounds,
            kept: -1,
            strikes: 0,
            last_activity: Instant::now(),
            last_retained_activity: None,
            retained_baseline_established: false,
            live_kept: -1,
        }
    }

    /// A request was answered: the idle period starts again.
    fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Extend the idle period and count growth from the first successfully retained project.
    pub fn note_retained_activity(&mut self, succeeded: bool) {
        self.last_retained_activity = Some(Instant::now());
        if succeeded && !self.retained_baseline_established {
            self.kept = -1;
            self.live_kept = -1;
            self.strikes = 0;
            self.retained_baseline_established = true;
        }
    }

    /// Whether the ordinary request idle period has elapsed.
    pub fn past_ordinary_idle(&self) -> bool {
        elapsed_millis(self.last_activity) >= self.bounds.idle_millis
    }

    /// Retire once both the ordinary idle period and any retained-work idle period have elapsed.
    pub fn idle(&self) -> bool {
        if !self.past_ordinary_idle() {
            return false;
        }
        match self.last_retained_activity {
            Some(last) => elapsed_millis(last) >= self.bounds.retained_idle_millis,
            None => true,
        }
    }

    /// The reply to an ended activation and whether the worker retires after it: an unrestored process retires at
    /// once; a panic is a compiler fault and the worker stays; the footprint and live heap decide the rest, an unknown
    /// footprint retiring nothing and a light activation held only to the ceiling. A rustc handoff is judged like any
    /// compile: the entry released the checking and lowering heap before it settled, so what the worker still holds
    /// decides whether it stays for the next edit.
    pub fn settle(&mut self, outcome: ActivationOutcome, footprint: i64, light: bool, live: i64) -> Settled {
        self.touch();
        let reply = match outcome {
            ActivationOutcome::UnsafeCleanup(reason) => {
                return Settled {
                    reply: Reply::CompilerFault(Fault::UnsafeCleanup(reason)),
                    retirement: Some(Retirement::Unsafe),
                };
            }
            ActivationOutcome::HandedOff(path) => Reply::HandedOff(path),
            ActivationOutcome::Exited(code) => Reply::Completed(code),
            ActivationOutcome::Unavailable(reason) => Reply::Refused(reason),
            ActivationOutcome::Panicked => Reply::CompilerFault(Fault::Panicked),
        };
        let retirement = self
            .over_footprint(footprint, light, live)
            .then_some(Retirement::Footprint);
        Settled { reply, retirement }
    }

    /// Whether a compile with this footprint after lowering hands rustc to its caller: a known one over the ceiling.
    pub fn hands_rustc_off(&self, footprint: i64) -> bool {
        footprint > self.bounds.ceiling
    }

    /// Whether an activation retires the worker: its footprint above the ceiling, or what it retains more than the
    /// margin above the least it has retained, for the last `FOOTPRINT_STRIKES` compiling activations.
    fn over_footprint(&mut self, footprint: i64, light: bool, live: i64) -> bool {
        if footprint < 0 {
            if !light {
                self.strikes = 0;
            }
            return false;
        }
        if light {
            return footprint > self.bounds.ceiling;
        }
        if self.retains_over_margin(footprint, live) {
            self.strikes += 1;
        } else {
            self.strikes = 0;
        }
        footprint > self.bounds.ceiling || self.strikes >= FOOTPRINT_STRIKES
    }

    /// Whether a compile left more than the margin above the least the worker has retained after one: in its live
    /// heap when that is known, else in its footprint. A later compile that left less lowers what is kept.
    fn retains_over_margin(&mut self, footprint: i64, live: i64) -> bool {
        let mut over = self.kept >= 0 && footprint - self.kept > self.bounds.margin;
        if self.kept < 0 || footprint < self.kept {
            self.kept = footprint;
        }
        if live < 0 {
            return over;
        }
        over = self.live_kept >= 0 && live - self.live_kept > self.bounds.margin;
        if self.live_kept < 0 || live < self.live_kept {
            self.live_kept = live;
        }
        over
    }
}

/// A worker's policy state, its idle period starting now.
pub fn lifecycle(bounds: Bounds) -> Lifecycle {
    Lifecycle::start(bounds)
}
