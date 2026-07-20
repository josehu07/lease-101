//! Shared simulation view layer: the pieces of the animated canvas that both the
//! `/sim` playground and the walkthrough's hardcoded scenario canvases render, so
//! the two look identical. The playground wraps these with scenario-editing
//! controls; a `ScenarioCanvas` wraps them with a fixed title and a stop
//! condition. See `crate::playground` and `crate::scenarios`.
//!
//! What lives here: the run lifecycle [`RunPhase`], a [`Topology`] describing the
//! grantor → grantee shape, the animated [`SimStage`] canvas, the [`RunBar`]
//! (buttons + timeline + status), the per-node [`GrantBars`], and all the
//! view-pure geometry/color helpers behind them.

use std::collections::BTreeSet;
use std::time::Duration;

use dioxus::prelude::*;
use lease_sim::frame::{lerp, ring_layout};
use lease_sim::{
    Engine, Event, Frame, LeaseStatus, MsgFate, MsgKind, NodeId, Point, Scenario, Time,
};

/// Global ticks between recorded frames — the time resolution of a run and the
/// granularity the timeline slider scrubs at. Kept fine for smooth motion.
pub const FRAME_TICKS: Time = 3;
/// Wall-clock interval between generation repaints (~143 fps), so message motion
/// stays smooth on high-refresh (120/144 Hz) displays rather than juddering at
/// the old ~83 fps. Paired with `FRAMES_PER_STEP = 1` so exactly one recorded
/// frame is painted per repaint. The *display* rate rises without much changing
/// the *simulation* rate: `FRAME_TICKS` drops with `RENDER_MS` so their ratio —
/// the sim-ticks-per-real-ms playback speed — stays close (`5/12 ≈ 0.417` was the
/// old pace; `3/7 ≈ 0.429` now, a hair faster).
pub const RENDER_MS: u32 = 7;
/// Frames advanced per repaint while generating. One frame per repaint keeps
/// display and sim resolution in lockstep.
const FRAMES_PER_STEP: usize = 1;
/// Frames-per-repaint multiplier applied while the fast-forward toggle is on.
const FF_MULT: usize = 3;

/// Lifecycle of a recorded/scrubbable run, shared by the playground and the
/// scenario canvases. The playground never leaves `Generating` on its own (it
/// caps out into `Stopped`); a scenario canvas transitions to `Stopped` when it
/// reaches its scripted stop tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    /// No run; the canvas shows the static topology.
    Idle,
    /// A run is being generated live (the simulation is playing out).
    Generating,
    /// Paused mid-generation; scrubbable and resumable.
    Paused,
    /// The run finished (hit its stop condition or cap); scrubbable, not
    /// resumable. A fresh Play/Restart is needed to run again.
    Stopped,
}

/// Strict majority (quorum-intersection threshold) for a cluster of size `n`.
pub fn majority(n: usize) -> usize {
    n / 2 + 1
}

/// When a run stops generating on its own. Every variant also carries a hard tick
/// cap (`at_tick` / the `cap` field) so a run can never generate forever even if
/// its event never fires.
#[derive(Clone, Copy)]
pub enum StopWhen {
    /// Stop at a fixed global tick (the playground's `MAX_TICKS`, or a scenario
    /// that just wants a time bound).
    AtTick(Time),
    /// Stop `after` ticks past the `n`-th event matching `pred`, or at `cap`
    /// ticks — whichever comes first. Lets a walkthrough scenario halt on a
    /// meaningful protocol milestone (e.g. "the grantor's 2nd renew reply", or
    /// "shortly after a guard ack is dropped") rather than a hand-tuned tick.
    /// `after` is a lead-out so the aftermath of the milestone stays on screen;
    /// `after: 0` stops right on the event.
    OnNthEvent {
        n: usize,
        pred: fn(&Event) -> bool,
        after: Time,
        cap: Time,
    },
}

impl StopWhen {
    /// The hard tick cap for this condition — the latest tick generation may run
    /// to regardless of events.
    fn cap(&self) -> Time {
        match self {
            StopWhen::AtTick(t) => *t,
            StopWhen::OnNthEvent { cap, .. } => *cap,
        }
    }
}

/// A live simulation run: the signals holding the recorded frames and lifecycle,
/// plus the wall-clock generation loop that fills them. Both the playground and
/// the walkthrough's scenario canvases drive their canvas through one of these,
/// so the record → scrub → resume machinery lives in exactly one place.
///
/// Construct with [`use_sim_run`] (installs the generation loop as a
/// `use_future`), then `start` a `Scenario`, `pause`/`resume`, `restart`, and
/// read the current frame via [`SimRun::frame`]. A run generates until it is
/// paused or its [`StopWhen`] fires, at which point it becomes
/// [`RunPhase::Stopped`].
#[derive(Clone, Copy)]
pub struct SimRun {
    phase: Signal<RunPhase>,
    engine: Signal<Option<Engine>>,
    frames: Signal<Vec<Frame>>,
    /// Global time of the next frame to generate.
    next_t: Signal<Time>,
    /// This run's stop condition (an event milestone or a fixed tick), always
    /// bounded by a tick cap.
    stop: Signal<StopWhen>,
    /// Count of stop-events seen so far this run (for `OnNthEvent`).
    events_seen: Signal<usize>,
    /// For `OnNthEvent`: once the n-th matching event fires, the tick generation
    /// should stop at (`event tick + after`, clamped to the cap). `None` until
    /// then; the run keeps going past the event so its aftermath stays on screen.
    event_deadline: Signal<Option<Time>>,
    cursor: Signal<usize>,
    fast_forward: Signal<bool>,
}

impl SimRun {
    /// (Re)build the run around `scenario`, generating from tick 0 until `stop`
    /// fires (an event milestone or a fixed tick, always tick-capped).
    pub fn start(&mut self, scenario: Scenario, stop: StopWhen) {
        self.engine.set(Some(Engine::new(scenario)));
        self.frames.write().clear();
        self.cursor.set(0);
        self.next_t.set(0);
        self.stop.set(stop);
        self.events_seen.set(0);
        self.event_deadline.set(None);
        self.phase.set(RunPhase::Generating);
    }

    /// Halt an in-progress run at the current frame, keeping the engine so
    /// [`resume`](Self::resume) can pick it back up.
    pub fn pause(&mut self) {
        if *self.phase.peek() == RunPhase::Generating {
            self.phase.set(RunPhase::Paused);
        }
    }

    /// Continue a paused run from where it left off.
    pub fn resume(&mut self) {
        if *self.phase.peek() == RunPhase::Paused {
            self.phase.set(RunPhase::Generating);
        }
    }

    /// Discard the run entirely, returning to `Idle` (static topology). The next
    /// `start` begins fresh.
    pub fn reset(&mut self) {
        self.phase.set(RunPhase::Idle);
        self.engine.set(None);
        self.frames.write().clear();
        self.next_t.set(0);
        self.events_seen.set(0);
        self.event_deadline.set(None);
        self.cursor.set(0);
    }

    pub fn phase(&self) -> RunPhase {
        (self.phase)()
    }

    /// Number of recorded frames so far.
    pub fn rec_len(&self) -> usize {
        self.frames.read().len()
    }

    /// The scrub cursor, clamped to the recorded range.
    pub fn cursor(&self) -> usize {
        (self.cursor)().min(self.rec_len().saturating_sub(1))
    }

    /// Move the scrub cursor (used by the timeline slider).
    pub fn set_cursor(&mut self, v: usize) {
        self.cursor.set(v);
    }

    pub fn fast_forward(&self) -> bool {
        (self.fast_forward)()
    }

    pub fn toggle_fast_forward(&mut self) {
        self.fast_forward.toggle();
    }

    /// The frame currently under the cursor, if any (`None` while idle).
    pub fn frame(&self) -> Option<Frame> {
        self.frames.read().get(self.cursor()).cloned()
    }

    /// Grant bars for `topology` over the recorded frames.
    pub fn grant_bars(&self, topology: &Topology) -> Vec<Vec<GrantRun>> {
        grant_bars(topology, &self.frames.read())
    }

    /// Cursor position as a `0..1` fraction of the run, for the grant-bar playhead.
    pub fn cursor_frac(&self) -> f64 {
        let len = self.rec_len();
        if len > 1 {
            self.cursor() as f64 / (len - 1) as f64
        } else {
            0.0
        }
    }

    /// Current scrub time in global ticks.
    pub fn now_ticks(&self) -> Time {
        self.cursor() as Time * FRAME_TICKS
    }

    /// The run's recorded end in global ticks.
    pub fn end_ticks(&self) -> Time {
        (self.rec_len().saturating_sub(1)) as Time * FRAME_TICKS
    }
}

/// Install a [`SimRun`]: the run signals plus the shared wall-clock generation
/// loop (a `use_future`), repainting every [`RENDER_MS`]. Call once at the top of
/// a component that hosts a canvas; the returned handle is `Copy`, so it can be
/// moved into event handlers.
pub fn use_sim_run() -> SimRun {
    let run = SimRun {
        phase: use_signal(|| RunPhase::Idle),
        engine: use_signal(|| None::<Engine>),
        frames: use_signal(Vec::<Frame>::new),
        next_t: use_signal(|| 0 as Time),
        stop: use_signal(|| StopWhen::AtTick(0)),
        events_seen: use_signal(|| 0usize),
        event_deadline: use_signal(|| None::<Time>),
        cursor: use_signal(|| 0usize),
        fast_forward: use_signal(|| false),
    };
    let SimRun {
        mut phase,
        mut engine,
        mut frames,
        mut next_t,
        stop,
        mut events_seen,
        mut event_deadline,
        mut cursor,
        fast_forward,
    } = run;

    // Generation loop: while `Generating`, advance the engine a batch of frames
    // per repaint, recording each. The run ends — dropping the engine but keeping
    // the recorded frames for scrubbing — when its `StopWhen` fires: either the
    // tick cap is reached, or (for `OnNthEvent`) the n-th matching event is
    // produced within the batch's `advance_to`.
    use_future(move || async move {
        loop {
            gloo_timers::future::sleep(Duration::from_millis(RENDER_MS as u64)).await;
            if *phase.peek() != RunPhase::Generating {
                continue;
            }
            let stop = *stop.peek();
            let cap = stop.cap();
            let mut last_idx = None;
            let mut done = false;
            let batch = FRAMES_PER_STEP * if *fast_forward.peek() { FF_MULT } else { 1 };
            for _ in 0..batch {
                let t = *next_t.peek();
                let frame = {
                    let mut eng = engine.write();
                    match eng.as_mut() {
                        Some(e) => {
                            // Events produced advancing into this frame's window;
                            // scanned for the stop milestone below.
                            let events = e.advance_to(t);
                            if let StopWhen::OnNthEvent { n, pred, after, .. } = stop {
                                // Once the n-th matching event fires, arm a deadline
                                // `after` ticks later (clamped to the cap) rather
                                // than stopping on the spot — so the milestone's
                                // aftermath stays on screen. Only the first arming
                                // sticks (`get_or_insert`).
                                if event_deadline.peek().is_none() {
                                    let hits = events.iter().filter(|ev| pred(ev)).count();
                                    if hits > 0 {
                                        let seen = *events_seen.peek() + hits;
                                        events_seen.set(seen);
                                        if seen >= n {
                                            event_deadline.set(Some((t + after).min(cap)));
                                        }
                                    }
                                }
                            }
                            e.frame_at(t)
                        }
                        None => break,
                    }
                };
                // Stop at the hard tick cap, or once the event deadline (armed
                // when the milestone fired) is reached.
                let deadline = event_deadline.peek().unwrap_or(cap).min(cap);
                if t >= deadline {
                    done = true;
                } else {
                    next_t.set(t + FRAME_TICKS);
                }
                let mut fr = frames.write();
                fr.push(frame);
                last_idx = Some(fr.len() - 1);
                if done {
                    break;
                }
            }
            if let Some(idx) = last_idx {
                cursor.set(idx);
            }
            if done {
                phase.set(RunPhase::Stopped);
                engine.set(None);
            }
        }
    });

    run
}

/// Generate the *entire* recorded frame sequence for a scenario up front, the
/// offline counterpart to the live [`use_sim_run`] loop (which fills the same
/// frames incrementally, one repaint-batch at a time). Produces byte-for-byte the
/// same `Vec<Frame>` the live canvas would record — same `FRAME_TICKS` stepping,
/// same `StopWhen` stop logic — so an offline consumer (the GIF capture route)
/// replays exactly what a viewer sees in the browser. Kept in lockstep with the
/// stop handling in `use_sim_run`.
pub fn generate_frames(scenario: Scenario, stop: StopWhen) -> Vec<Frame> {
    let mut eng = Engine::new(scenario);
    let cap = stop.cap();
    let mut frames = Vec::new();
    let mut t: Time = 0;
    let mut events_seen = 0usize;
    let mut deadline: Option<Time> = None;
    loop {
        let events = eng.advance_to(t);
        if let StopWhen::OnNthEvent { n, pred, after, .. } = stop
            && deadline.is_none()
        {
            let hits = events.iter().filter(|ev| pred(ev)).count();
            if hits > 0 {
                events_seen += hits;
                if events_seen >= n {
                    deadline = Some((t + after).min(cap));
                }
            }
        }
        frames.push(eng.frame_at(t));
        let d = deadline.unwrap_or(cap).min(cap);
        if t >= d {
            break;
        }
        t += FRAME_TICKS;
    }
    frames
}

/// The grantor → grantee shape of a scenario: everything the static canvas and
/// the per-node bar layout derive from, independent of any running simulation.
#[derive(Debug, Clone, PartialEq)]
pub struct Topology {
    pub n: usize,
    pub grantors: BTreeSet<usize>,
    pub grantees: BTreeSet<usize>,
}

impl Topology {
    /// Build from a node count and grantor/grantee id lists.
    pub fn new(
        n: usize,
        grantors: impl IntoIterator<Item = usize>,
        grantees: impl IntoIterator<Item = usize>,
    ) -> Self {
        Self {
            n,
            grantors: grantors.into_iter().collect(),
            grantees: grantees.into_iter().collect(),
        }
    }

    /// Derive the topology from a scenario's declared leases, so a hardcoded demo
    /// has a single source of truth (the `Scenario`) for both its run and its
    /// static shape.
    pub fn from_scenario(s: &Scenario) -> Self {
        Self {
            n: s.node_count(),
            grantors: s.leases.iter().map(|l| l.grantor).collect(),
            grantees: s.leases.iter().map(|l| l.grantee).collect(),
        }
    }

    /// Whether at least one grantor → grantee lease is declared (`g != h`).
    pub fn has_leases(&self) -> bool {
        self.grantors.iter().any(|&g| {
            self.grantees
                .iter()
                .any(|&h| g != h && g < self.n && h < self.n)
        })
    }

    /// The "leader": the smallest-id grantee, marked with a crown. `None` if
    /// there is no in-range grantee.
    pub fn leader(&self) -> Option<usize> {
        self.grantees.iter().copied().find(|&id| id < self.n)
    }

    /// Per-node `(out, inc)` bar counts: leases it grants (as-grantor) and leases
    /// it holds (as-grantee), the same pairs a scenario declares (`g != h`).
    pub fn bar_counts(&self) -> Vec<(usize, usize)> {
        (0..self.n)
            .map(|node| {
                let out = if self.grantors.contains(&node) {
                    self.grantees.iter().filter(|&&h| h != node).count()
                } else {
                    0
                };
                let inc = if self.grantees.contains(&node) {
                    self.grantors.iter().filter(|&&g| g != node).count()
                } else {
                    0
                };
                (out, inc)
            })
            .collect()
    }

    /// Per-node implicit self-grant (1 if it is both a grantor and a grantee).
    pub fn self_grant(&self) -> Vec<usize> {
        (0..self.n)
            .map(|node| usize::from(self.grantors.contains(&node) && self.grantees.contains(&node)))
            .collect()
    }

    /// Per-node max grants it can hold as a grantee: grantors that grant to it
    /// plus its own self-grant; 0 for a non-grantee.
    pub fn max_grants(&self) -> Vec<usize> {
        let self_grant = self.self_grant();
        (0..self.n)
            .map(|node| {
                if self.grantees.contains(&node) {
                    self.grantors.iter().filter(|&&g| g != node).count() + self_grant[node]
                } else {
                    0
                }
            })
            .collect()
    }
}

// ---- Edge geometry ----------------------------------------------------------

/// A directed lease edge in unit-canvas coordinates, endpoints pulled in so the
/// arrowhead lands at the node's border rather than its center.
#[derive(Debug, Clone, Copy)]
struct Edge {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

/// Geometry of the directed segment `a -> b`, inset at both ends to clear the
/// node disks.
fn edge_between(a: Point, b: Point) -> Edge {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    let (ux, uy) = (dx / len, dy / len);
    let tail_inset = 0.075; // start end, pulled a bit further off its node
    let tip_inset = 0.055; // arrowhead end, at the target node's border
    Edge {
        x1: a.x + ux * tail_inset,
        y1: a.y + uy * tail_inset,
        x2: b.x - ux * tip_inset,
        y2: b.y - uy * tip_inset,
    }
}

/// How far the arrowhead marker extends back from the tip, in unit-canvas
/// coordinates: `(ref_x / viewBox_width) · marker_width` = `(8/10)·0.032`.
const ARROW_LEN: f64 = 0.0256;

/// `e` with its tip pulled back by [`ARROW_LEN`], so the drawn stem ends at the
/// arrowhead base instead of running under the head.
fn stem(e: Edge) -> Edge {
    let (dx, dy) = (e.x2 - e.x1, e.y2 - e.y1);
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    let (ux, uy) = (dx / len, dy / len);
    Edge {
        x2: e.x2 - ux * ARROW_LEN,
        y2: e.y2 - uy * ARROW_LEN,
        ..e
    }
}

// ---- Timer-bar layout geometry ---------------------------------------------
// Each node's countdown stack is pushed radially into the ring's *outer* band.
// The stack is a small two-column grid — an OUT column (as-grantor bars) beside
// an IN column (as-grantee bars), each capped by a header cell — so its row count
// is `max(out, in)`. Its bars are rem-sized (physical) while node positions are
// unit-canvas fractions, so two things adapt to a node's `(out, inc)` counts and
// radial direction: its push distance and the whole ring's radius. All sizes
// mirror the CSS.

/// One rem as a fraction of the stage box (conservative 520px assumption).
const REM_UNIT: f64 = 17.0 / 520.0;
/// Half the 2.1rem node disk (`.pg-node`).
const DISK_R_REM: f64 = 1.05;
/// One grid row tall (`.pg-timer-cell`) plus the inter-row gap.
const ROW_H_REM: f64 = 0.7;
const ROW_GAP_REM: f64 = 0.18;
/// One column wide: the `→N` row-label + its gap to the bar + the bar.
const COL_W_REM: f64 = 0.95 + 0.26 + 1.9;
const COL_GAP_REM: f64 = 0.45;
/// Breathing room between a disk and its stack.
const CLEAR_REM: f64 = 0.45;
/// Keep every bar at least this far (unit) inside the stage edge.
const RING_MARGIN: f64 = 0.015;
/// The radius `frame::ring_layout` places nodes at; the ring scales relative to
/// it, never collapsing below `MIN_SCALE` of it.
const BASE_RING_R: f64 = 0.38;
const MIN_SCALE: f64 = 0.45;
/// Minimum ring scale in `fit_height` (walkthrough) mode. Higher than `MIN_SCALE`
/// so busy topologies (e.g. quorum/roster holders with wide two-column timer
/// stacks) keep their nodes spread out like the sparser leader canvas, rather
/// than clustering toward center. The stacks then sit nearer the stage edge — the
/// fit-height box simply grows (taller/wider) to accommodate them, which is fine
/// for the blog canvases. `node_max_scale`'s stage-edge fit assumes a conservative
/// 520px box (`REM_UNIT`); the real stage is wider, so this floor doesn't clip in
/// practice.
const FIT_MIN_SCALE: f64 = 0.78;

/// Vertical squish applied to the ring layout in `fit_height` (walkthrough)
/// mode for clusters larger than 2: node `y` offsets from center are scaled by
/// this, flattening the circle into a short/fat ellipse so the blog-post canvas
/// takes less vertical space (horizontal spread untouched). 1.0 = a plain circle.
const FIT_Y_SQUISH: f64 = 0.52;

/// Vertical clearance (rem) a leader node needs above its disk center for the
/// crown perched over it (crown top sits ~2.0rem above center).
const CROWN_UP_REM: f64 = 2.05;
/// Extra vertical padding (rem) around a disk for its aura halo.
const AURA_PAD_REM: f64 = 0.5;

/// Row count of a node's two-column stack: the taller of its OUT / IN columns.
fn stack_rows(out: usize, inc: usize) -> usize {
    out.max(inc)
}

/// Column count: 1 if the node is only a grantor or only a grantee, 2 if both.
fn stack_cols(out: usize, inc: usize) -> usize {
    usize::from(out > 0) + usize::from(inc > 0)
}

/// Half the stack's height (rem): its bar rows plus the one header row.
fn stack_half_h_rem(out: usize, inc: usize) -> f64 {
    let rows = stack_rows(out, inc);
    if rows == 0 {
        return 0.0;
    }
    let tall = (rows + 1) as f64; // + header row
    (tall * ROW_H_REM + (tall - 1.0) * ROW_GAP_REM) / 2.0
}

/// Half the stack's width (rem): its 1 or 2 columns and the gap between them.
fn stack_half_w_rem(out: usize, inc: usize) -> f64 {
    let cols = stack_cols(out, inc) as f64;
    if cols == 0.0 {
        return 0.0;
    }
    (cols * COL_W_REM + (cols - 1.0) * COL_GAP_REM) / 2.0
}

/// Radial push (rem) that clears a node's stack off its disk when pushed along
/// the unit direction `(ux, uy)`.
fn timer_offset_rem(out: usize, inc: usize, ux: f64, uy: f64) -> f64 {
    if stack_rows(out, inc) == 0 {
        return 0.0;
    }
    let along = ux.abs() * stack_half_w_rem(out, inc) + uy.abs() * stack_half_h_rem(out, inc);
    DISK_R_REM + CLEAR_REM + along
}

/// Largest ring scale `k` (fraction of `BASE_RING_R`) at which this node's stack
/// box stays within the stage margins.
fn node_max_scale(ux: f64, uy: f64, out: usize, inc: usize) -> f64 {
    if stack_rows(out, inc) == 0 {
        return f64::INFINITY;
    }
    let off = timer_offset_rem(out, inc, ux, uy) * REM_UNIT;
    let hw = stack_half_w_rem(out, inc) * REM_UNIT;
    let hh = stack_half_h_rem(out, inc) * REM_UNIT;
    let (lo, hi) = (RING_MARGIN, 1.0 - RING_MARGIN);
    let axis = |u: f64, half: f64| -> f64 {
        if u > 0.0 {
            (hi - half - 0.5 - off * u) / (BASE_RING_R * u)
        } else if u < 0.0 {
            (lo + half - 0.5 - off * u) / (BASE_RING_R * u)
        } else {
            f64::INFINITY
        }
    };
    axis(ux, hw).min(axis(uy, hh))
}

/// The vertical band `[y_min, y_max]` (unit coords) the layout occupies: every
/// node disk, its aura pad, a leader's crown, and its timer stack's half-height.
/// Used to *crop* a fit-height stage to its content — the returned band maps to
/// the full stage height at the same (isotropic) pixel scale as the horizontal
/// axis, so nothing is squashed. Fed the *unscaled* (full-radius) node centers so
/// the box stays a fixed height across idle/running; see the call site.
fn content_y_band(
    pts: &[Point],
    bar_counts: &[(usize, usize)],
    leader: Option<usize>,
) -> (f64, f64) {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (id, p) in pts.iter().enumerate() {
        let (out, inc) = bar_counts[id];
        // Half-height (rem) this node reaches above/below its center: the tallest
        // of its own disk (+aura), its timer stack's half-height, and — above only,
        // on the leader — the crown's reach (`CROWN_UP_REM` is the crown top's
        // distance *above center*, so it's a `max`, not an addition on top of the
        // disk).
        let stack_hh = stack_half_h_rem(out, inc);
        let disk_hh = DISK_R_REM + AURA_PAD_REM;
        let down_rem = disk_hh.max(stack_hh);
        let up_rem = if leader == Some(id) {
            down_rem.max(CROWN_UP_REM)
        } else {
            down_rem
        };
        lo = lo.min(p.y - up_rem * REM_UNIT);
        hi = hi.max(p.y + down_rem * REM_UNIT);
    }
    if !lo.is_finite() {
        return (0.0, 1.0);
    }
    // A hair of breathing room, and never invert.
    lo -= RING_MARGIN;
    hi += RING_MARGIN;
    (lo.min(hi - 1e-3), hi)
}

/// A lease edge to draw during playback, colored by the grantee's held status.
#[derive(Debug, Clone, Copy)]
struct LeaseEdge {
    e: Edge,
    status: LeaseStatus,
    /// Grantee's remaining lease life, `0.0..1.0`, for a countdown fade.
    fill: f64,
}

/// Which side of a lease a countdown bar reflects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimerRole {
    Grantor,
    Grantee,
}

/// One countdown timer bar drawn beside a node disk during playback.
#[derive(Debug, Clone, Copy)]
struct TimerBar {
    /// The lease's other endpoint — named in the hover tooltip.
    other: NodeId,
    role: TimerRole,
    status: LeaseStatus,
    fill: f64,
}

/// One run-length run of a node's grant history: it held `grants` active grants
/// as a grantee for `frames` consecutive recorded frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrantRun {
    frames: usize,
    grants: usize,
}

/// Active grants held by `node` as a grantee in one frame.
fn grant_count(f: &Frame, node: NodeId) -> usize {
    f.leases
        .iter()
        .filter(|b| b.grantee == node && b.grantee_status == LeaseStatus::Active)
        .count()
}

/// Countdown bars for one node in a frame: grantor bars first, then grantee bars.
fn node_timers(f: &Frame, node: NodeId) -> Vec<TimerBar> {
    let grantor = f
        .leases
        .iter()
        .filter(|b| b.grantor == node)
        .map(|b| TimerBar {
            other: b.grantee,
            role: TimerRole::Grantor,
            status: b.grantor_status,
            fill: b.grantor_fill,
        });
    let grantee = f
        .leases
        .iter()
        .filter(|b| b.grantee == node)
        .map(|b| TimerBar {
            other: b.grantor,
            role: TimerRole::Grantee,
            status: b.grantee_status,
            fill: b.grantee_fill,
        });
    grantor.chain(grantee).collect()
}

/// Run-length encode a node's active-grant count over the recorded `frames`.
/// `base` is the node's implicit self-grant.
fn grant_runs(frames: &[Frame], node: NodeId, base: usize) -> Vec<GrantRun> {
    if frames.is_empty() {
        return vec![GrantRun {
            frames: 1,
            grants: base,
        }];
    }
    let mut runs: Vec<GrantRun> = Vec::new();
    for f in frames {
        let grants = grant_count(f, node) + base;
        match runs.last_mut() {
            Some(r) if r.grants == grants => r.frames += 1,
            _ => runs.push(GrantRun { frames: 1, grants }),
        }
    }
    runs
}

/// Grant bars for every node in a topology over a run's recorded frames.
pub fn grant_bars(topology: &Topology, frames: &[Frame]) -> Vec<Vec<GrantRun>> {
    let self_grant = topology.self_grant();
    (0..topology.n)
        .map(|node| grant_runs(frames, node, self_grant[node]))
        .collect()
}

/// Plural suffix for a count: "" for 1, "s" otherwise.
fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Grantee green (`--grantee`, `#2f8f5b`) and the surface gray (`--surface`,
/// `#f4f5f7`) as RGB triples, so the grant-bar/aura shading mixes match the CSS.
const LEASE_GREEN: (f64, f64, f64) = (0x2f as f64, 0x8f as f64, 0x5b as f64);
const SURFACE_GRAY: (f64, f64, f64) = (0xf4 as f64, 0xf5 as f64, 0xf7 as f64);

/// Mix color `c` `amt` of the way from `toward` (per channel), formatted as a
/// comma-free `#rrggbb` string. (Comma-free on purpose: Dioxus's inline `style`
/// parser drops values containing commas, so `color-mix(...)` can't be used.)
fn mix_hex(c: (f64, f64, f64), amt: f64, toward: (f64, f64, f64)) -> String {
    let mix = |x: f64, t: f64| (x * amt + t * (1.0 - amt)).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        mix(c.0, toward.0),
        mix(c.1, toward.1),
        mix(c.2, toward.2),
    )
}

/// CSS color for a grant-status run: an empty track (surface gray) when no grant
/// is held, else grantee green mixed toward white, deepening with `grants`/`max`.
fn grant_color(grants: usize, max: usize) -> String {
    if grants == 0 || max == 0 {
        return "#f4f5f7".to_string(); // matches --surface (empty track)
    }
    let frac = (grants as f64 / max as f64).clamp(0.0, 1.0);
    let green = 0.30 + 0.55 * frac; // 30%..85% green, remainder white
    mix_hex(LEASE_GREEN, green, (255.0, 255.0, 255.0))
}

/// Inline style for a node's green "aura" halo, sized/shaded by `frac`.
fn aura_style(frac: f64) -> String {
    if frac <= 0.0 {
        return String::new();
    }
    let frac = frac.clamp(0.0, 1.0);
    let spread = 2.0 + 2.5 * frac;
    let blur = 1.0 + 2.0 * frac;
    let green = 0.35 + 0.5 * frac;
    let color = mix_hex(LEASE_GREEN, green, SURFACE_GRAY);
    format!("--aura-blur: {blur:.2}px; --aura-spread: {spread:.2}px; --aura-color: {color};")
}

/// Arrowhead marker id for a lease edge in playback, colored to match its stem.
fn ledge_marker(status: LeaseStatus) -> &'static str {
    match status {
        LeaseStatus::Active => "url(#pg-arrow-active)",
        LeaseStatus::Guarding => "url(#pg-arrow-guarding)",
        _ => "url(#pg-arrow-idle)",
    }
}

/// Opacity of a lease edge (stem and head share it): an active lease fades with
/// its remaining life `fill` for a visible countdown, guarding is a steady
/// half-opacity, anything idle is faint.
fn ledge_opacity(status: LeaseStatus, fill: f64) -> f64 {
    match status {
        LeaseStatus::Active => 0.4 + 0.55 * fill,
        LeaseStatus::Guarding => 0.5,
        _ => 0.1,
    }
}

/// CSS class for a message glyph, grouped (colored) by protocol phase.
fn msg_class(kind: MsgKind) -> &'static str {
    match kind {
        MsgKind::Guard => "pg-msg is-guard",
        MsgKind::GuardReply => "pg-msg is-guard is-reply",
        MsgKind::Renew => "pg-msg is-renew",
        MsgKind::RenewReply => "pg-msg is-renew is-reply",
        MsgKind::Revoke => "pg-msg is-revoke",
        MsgKind::RevokeReply => "pg-msg is-revoke is-reply",
        MsgKind::Write => "pg-msg is-write",
        MsgKind::WriteReply => "pg-msg is-write is-reply",
        MsgKind::Commit => "pg-msg is-commit",
    }
}

/// Whether a message is an acknowledgement reply.
fn msg_is_reply(kind: MsgKind) -> bool {
    matches!(
        kind,
        MsgKind::GuardReply | MsgKind::RenewReply | MsgKind::RevokeReply | MsgKind::WriteReply
    )
}

/// Opacity of an in-flight message glyph at flight `progress` (`0.0..1.0`).
fn msg_opacity(progress: f64) -> f64 {
    const FADE: f64 = 0.25;
    if progress < FADE {
        (progress / FADE).clamp(0.0, 1.0)
    } else if progress > 1.0 - FADE {
        ((1.0 - progress) / FADE).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// Flight `progress` at which a *dropped* message dies mid-link.
const DROP_AT: f64 = 0.45;
/// How much flight `progress` the red drop burst plays out over.
const BURST_DUR: f64 = 0.4;

/// Opacity of a *dropped* message's glyph at flight `progress`.
fn drop_glyph_opacity(progress: f64) -> f64 {
    const FADE_IN: f64 = 0.15;
    const FADE_OUT: f64 = 0.1;
    if progress >= DROP_AT {
        return 0.0;
    }
    if progress < FADE_IN {
        (progress / FADE_IN).clamp(0.0, 1.0)
    } else if progress > DROP_AT - FADE_OUT {
        ((DROP_AT - progress) / FADE_OUT).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// Burst animation parameter for a dropped message at flight `progress`.
fn drop_burst(progress: f64) -> Option<f64> {
    if progress < DROP_AT {
        return None;
    }
    let bp = (progress - DROP_AT) / BURST_DUR;
    (bp <= 1.0).then_some(bp.clamp(0.0, 1.0))
}

/// Symbol for an in-flight message, drawn inside its positioned `.pg-msg` chip.
#[component]
fn MsgGlyph(kind: MsgKind) -> Element {
    let base = match kind {
        MsgKind::Guard | MsgKind::GuardReply => rsx! {
            svg { class: "pg-msg-icon", view_box: "0 0 24 24",
                path { d: "M12 2.2 L19.5 5.2 V11 C19.5 16 16.2 20.2 12 21.6 C7.8 20.2 4.5 16 4.5 11 V5.2 Z" }
            }
        },
        MsgKind::Renew | MsgKind::RenewReply => rsx! {
            svg { class: "pg-msg-icon", view_box: "0 0 24 24",
                path {
                    transform: "rotate(180 12 12)",
                    d: "M17.65 6.35C16.2 4.9 14.21 4 12 4c-4.42 0-7.99 3.58-7.99 8s3.57 8 7.99 8c3.73 0 6.84-2.55 7.73-6h-2.08c-.82 2.33-3.04 4-5.65 4-3.31 0-6-2.69-6-6s2.69-6 6-6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z",
                }
            }
        },
        // Revoke / RevokeReply: a "prohibition" sign — a ring with a diagonal
        // slash — reading clearly as "lease cancelled", distinct from the Write
        // pencil glyph below.
        MsgKind::Revoke | MsgKind::RevokeReply => rsx! {
            svg { class: "pg-msg-icon", view_box: "0 0 24 24",
                circle {
                    cx: "12",
                    cy: "12",
                    r: "8.5",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2.2",
                }
                path {
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2.2",
                    stroke_linecap: "round",
                    d: "M6 6 L18 18",
                }
            }
        },
        MsgKind::Write | MsgKind::WriteReply => rsx! {
            svg { class: "pg-msg-icon", view_box: "0 0 24 24",
                path { d: "M3 17.25V21h3.75L17.81 9.94l-3.75-3.75L3 17.25zM20.71 7.04c.39-.39.39-1.02 0-1.41l-2.34-2.34a.9959.9959 0 0 0-1.41 0l-1.83 1.83 3.75 3.75 1.83-1.83z" }
            }
        },
        MsgKind::Commit => rsx! {
            svg { class: "pg-msg-icon", view_box: "0 0 24 24",
                path { d: "M9 16.17 L4.83 12 l-1.42 1.41 L9 19 L21 7 l-1.41-1.41 z" }
            }
        },
    };
    rsx! {
        {base}
        if msg_is_reply(kind) {
            span { class: "pg-msg-badge",
                svg { class: "pg-msg-badge-icon", view_box: "0 0 24 24",
                    path { d: "M1 21h4V9H1v12zm22-11c0-1.1-.9-2-2-2h-6.31l.95-4.57.03-.32c0-.41-.17-.79-.44-1.06L14.17 1 7.59 7.59C7.22 7.95 7 8.45 7 9v10c0 1.1.9 2 2 2h9c.83 0 1.54-.5 1.84-1.22l3.02-7.05c.09-.23.14-.47.14-.73v-2z" }
                }
            }
        }
    }
}

/// A small crown badge marking the "leader" node.
#[component]
fn Crown() -> Element {
    rsx! {
        svg { class: "pg-crown", view_box: "0 0 24 24",
            path { d: "M3 8 L6.5 13 L12 6 L17.5 13 L21 8 L19 19 L5 19 Z" }
        }
    }
}

/// The `.pg-stage` canvas: the topology backdrop, live lease edges, in-flight
/// message glyphs, node disks (with auras, crown, timer stacks) — or, when
/// `frame` is `None`, the static grantor → grantee topology.
///
/// With `fit_height` set, the stage box is cropped to the layout's actual
/// vertical extent (a short canvas for a horizontal 2-node scenario, say) rather
/// than kept square — the crop is *isotropic* (equal x/y pixel scale), so nothing
/// is squashed.
#[component]
pub fn SimStage(
    topology: Topology,
    frame: Option<Frame>,
    /// Crop the stage height to the content instead of a square box.
    #[props(default = false)]
    fit_height: bool,
    /// Extra vertical breathing room (unit fraction) added to *each* end of the
    /// fit-height crop band, on top of the content extent. Default 0; a scenario
    /// whose off-axis nodes carry tall pushed-out timer stacks (e.g. the all-to-all
    /// roster) sets a small value so those stacks aren't clipped, without changing
    /// the height of the sparser canvases. Ignored unless `fit_height`.
    #[props(default = 0.0)]
    fit_pad: f64,
) -> Element {
    let n = topology.n;
    let maj = majority(n);
    let leader = topology.leader();
    let bar_counts = topology.bar_counts();
    let self_grant = topology.self_grant();
    let max_grants = topology.max_grants();

    // Node positions. In the playback view the ring is adaptively shrunk toward
    // center so the busiest node's stack clears its disk and stays in the stage.
    // In fit-height (walkthrough) mode with more than 2 nodes, the ring is also
    // flattened vertically into a short/fat ellipse (`FIT_Y_SQUISH`) so the blog
    // canvas is more compact — horizontal spread is left untouched. Everything
    // downstream (ring-scale fit, edges, crop band, timer-stack push directions,
    // message paths) derives from `base_pts`, so the squish propagates cleanly.
    let base_pts: Vec<Point> = ring_layout(n)
        .into_iter()
        .map(|p| {
            if fit_height && n > 2 {
                Point {
                    x: p.x,
                    y: 0.5 + (p.y - 0.5) * FIT_Y_SQUISH,
                }
            } else {
                p
            }
        })
        .collect();
    let ring_scale = if frame.is_some() {
        // Fit-height canvases keep a higher floor so busy topologies stay spread
        // (their stacks sit nearer the edge and the box just grows); the square
        // playground uses the tighter floor so a pathological all-to-all still fits.
        let floor = if fit_height { FIT_MIN_SCALE } else { MIN_SCALE };
        base_pts
            .iter()
            .enumerate()
            .map(|(id, p)| {
                let (dx, dy) = (p.x - 0.5, p.y - 0.5);
                let len = (dx * dx + dy * dy).sqrt();
                if len < 1e-6 {
                    return f64::INFINITY;
                }
                let (out, inc) = bar_counts[id];
                node_max_scale(dx / len, dy / len, out, inc)
            })
            .fold(1.0_f64, f64::min)
            .clamp(floor, 1.0)
    } else {
        1.0
    };
    let pts: Vec<Point> = base_pts
        .iter()
        .map(|p| Point {
            x: 0.5 + (p.x - 0.5) * ring_scale,
            y: 0.5 + (p.y - 0.5) * ring_scale,
        })
        .collect();

    // Vertical crop window. In fit mode the stage is shortened to the content's
    // actual band (isotropically — the SVG viewBox and node `top%` are both
    // remapped through it), so a horizontal layout gets a short canvas. Otherwise
    // the full `[0, 1]` square box, and `ny` is the identity `y·100`.
    //
    // The band is computed from the *unscaled* `base_pts` (full ring radius), not
    // the possibly-shrunk `pts`, so the box height is identical whether idle or
    // running — the in-run ring shrink (for timer stacks) just repositions nodes
    // *within* this fixed box rather than resizing it. Timer stacks are accounted
    // for via `bar_counts` (known even while idle), and full radius is the run's
    // upper bound on vertical extent, so nothing overflows once the ring shrinks.
    let (y_lo, y_hi) = if fit_height {
        // Optional extra breathing room at each end, for scenarios whose off-axis
        // stacks would otherwise clip (see `fit_pad`).
        let (lo, hi) = content_y_band(&base_pts, &bar_counts, leader);
        (lo - fit_pad, hi + fit_pad)
    } else {
        (0.0, 1.0)
    };
    let y_span = (y_hi - y_lo).max(1e-3);
    // A unit y → CSS `top` percentage of the (possibly cropped) stage box.
    let ny = |y: f64| (y - y_lo) / y_span * 100.0;
    // SVG viewBox matches the crop, so unit-space line/edge coords need no change.
    let view_box = format!("0 {y_lo:.5} 1 {y_span:.5}");
    // Stage box: cropped aspect ratio (width : height = 1 : y_span) when fitting.
    let stage_style = if fit_height {
        format!("aspect-ratio: 1 / {y_span:.5};")
    } else {
        String::new()
    };

    // Static grantor → grantee arrows (gray topology backdrop / editing view).
    let edges: Vec<Edge> = {
        let mut v = Vec::new();
        for &g in topology.grantors.iter() {
            for &h in topology.grantees.iter() {
                if g == h || g >= n || h >= n {
                    continue;
                }
                v.push(edge_between(pts[g], pts[h]));
            }
        }
        v
    };

    // Playback-derived geometry.
    let lease_edges: Vec<LeaseEdge> = frame
        .as_ref()
        .map(|f| {
            f.leases
                .iter()
                .filter(|b| b.grantor < n && b.grantee < n)
                .map(|b| LeaseEdge {
                    e: edge_between(pts[b.grantor], pts[b.grantee]),
                    status: b.grantee_status,
                    fill: b.grantee_fill,
                })
                .collect()
        })
        .unwrap_or_default();
    let held: Vec<usize> = (0..n)
        .map(|node| {
            frame.as_ref().map_or(0, |f| {
                f.leases
                    .iter()
                    .filter(|b| b.grantee == node && b.grantee_status == LeaseStatus::Active)
                    .count()
                    + 1
            })
        })
        .collect();
    let aura: Vec<f64> = (0..n)
        .map(|node| {
            if max_grants[node] == 0 {
                return 0.0;
            }
            let held_as_grantee =
                frame.as_ref().map_or(0, |f| grant_count(f, node)) + self_grant[node];
            (held_as_grantee as f64 / max_grants[node] as f64).clamp(0.0, 1.0)
        })
        .collect();
    let timers: Vec<Vec<TimerBar>> = (0..n)
        .map(|node| {
            frame
                .as_ref()
                .map(|f| node_timers(f, node))
                .unwrap_or_default()
        })
        .collect();

    let grantors = &topology.grantors;
    let grantees = &topology.grantees;

    rsx! {
        div {
            class: if fit_height { "pg-stage is-fit" } else { "pg-stage" },
            style: "{stage_style}",
            if frame.is_some() {
                svg {
                    class: "pg-edges",
                    view_box: "{view_box}",
                    preserve_aspect_ratio: "none",
                    defs {
                        marker {
                            id: "pg-arrow-active",
                            view_box: "0 0 10 10",
                            ref_x: "8",
                            ref_y: "5",
                            marker_units: "userSpaceOnUse",
                            marker_width: "0.032",
                            marker_height: "0.032",
                            orient: "auto",
                            path {
                                class: "pg-arrow-head is-active",
                                d: "M0,0 L10,5 L0,10 z",
                            }
                        }
                        marker {
                            id: "pg-arrow-guarding",
                            view_box: "0 0 10 10",
                            ref_x: "8",
                            ref_y: "5",
                            marker_units: "userSpaceOnUse",
                            marker_width: "0.032",
                            marker_height: "0.032",
                            orient: "auto",
                            path {
                                class: "pg-arrow-head is-guarding",
                                d: "M0,0 L10,5 L0,10 z",
                            }
                        }
                        marker {
                            id: "pg-arrow-idle",
                            view_box: "0 0 10 10",
                            ref_x: "8",
                            ref_y: "5",
                            marker_units: "userSpaceOnUse",
                            marker_width: "0.032",
                            marker_height: "0.032",
                            orient: "auto",
                            path {
                                class: "pg-arrow-head is-idle",
                                d: "M0,0 L10,5 L0,10 z",
                            }
                        }
                        marker {
                            id: "pg-arrow",
                            view_box: "0 0 10 10",
                            ref_x: "8",
                            ref_y: "5",
                            marker_units: "userSpaceOnUse",
                            marker_width: "0.032",
                            marker_height: "0.032",
                            orient: "auto",
                            path {
                                class: "pg-arrow-head",
                                d: "M0,0 L10,5 L0,10 z",
                            }
                        }
                    }
                    for (i , e) in edges.iter().enumerate() {
                        {
                            let st = stem(*e);
                            rsx! {
                                line {
                                    key: "b{i}",
                                    class: "pg-edge",
                                    x1: "{st.x1}",
                                    y1: "{st.y1}",
                                    x2: "{st.x2}",
                                    y2: "{st.y2}",
                                }
                            }
                        }
                    }
                    for (i , e) in edges.iter().enumerate() {
                        line {
                            key: "bh{i}",
                            class: "pg-edge-head",
                            x1: "{e.x1}",
                            y1: "{e.y1}",
                            x2: "{e.x2}",
                            y2: "{e.y2}",
                            "marker-end": "url(#pg-arrow)",
                        }
                    }
                    for (i , le) in lease_edges.iter().enumerate() {
                        {
                            let st = stem(le.e);
                            rsx! {
                                line {
                                    key: "s{i}",
                                    class: match le.status {
                                        LeaseStatus::Active => "pg-ledge is-active",
                                        LeaseStatus::Guarding => "pg-ledge is-guarding",
                                        _ => "pg-ledge is-idle",
                                    },
                                    x1: "{st.x1}",
                                    y1: "{st.y1}",
                                    x2: "{st.x2}",
                                    y2: "{st.y2}",
                                    opacity: ledge_opacity(le.status, le.fill),
                                }
                            }
                        }
                    }
                    for (i , le) in lease_edges.iter().enumerate() {
                        line {
                            key: "h{i}",
                            class: "pg-ledge-head",
                            x1: "{le.e.x1}",
                            y1: "{le.e.y1}",
                            x2: "{le.e.x2}",
                            y2: "{le.e.y2}",
                            opacity: ledge_opacity(le.status, le.fill),
                            "marker-end": ledge_marker(le.status),
                        }
                    }
                }
                if let Some(f) = frame.as_ref() {
                    for (i , m) in f.messages.iter().enumerate() {
                        {
                            let opacity = match m.fate {
                                MsgFate::Dropped => drop_glyph_opacity(m.progress),
                                MsgFate::Delivered => msg_opacity(m.progress),
                            };
                            let p = lerp(pts[m.from], pts[m.to], m.progress);
                            rsx! {
                                div {
                                    key: "m{i}",
                                    class: msg_class(m.kind),
                                    style: format!("left: {:.3}%; top: {:.3}%; opacity: {:.3};", p.x * 100.0, ny(p.y), opacity),
                                    MsgGlyph { kind: m.kind }
                                }
                            }
                        }
                        if m.fate == MsgFate::Dropped {
                            if let Some(bp) = drop_burst(m.progress) {
                                {
                                    let p = lerp(pts[m.from], pts[m.to], DROP_AT);
                                    rsx! {
                                        div {
                                            key: "d{i}",
                                            class: "pg-drop",
                                            style: format!("left: {:.3}%; top: {:.3}%; --bp: {:.3};", p.x * 100.0, ny(p.y), bp),
                                            span { class: "pg-drop-ring" }
                                            svg { class: "pg-drop-x", view_box: "0 0 24 24",
                                                path { d: "M6 6 L18 18 M18 6 L6 18" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                for id in 0..n {
                    div {
                        key: "{id}",
                        class: {
                            let mut c = String::from("pg-node");
                            if grantors.contains(&id) {
                                c.push_str(" is-grantor");
                            }
                            if grantees.contains(&id) {
                                c.push_str(" is-grantee");
                            }
                            if held[id] >= maj {
                                c.push_str(" is-majority");
                            }
                            c
                        },
                        style: format!(
                            "left: {:.3}%; top: {:.3}%; {}",
                            pts[id].x * 100.0,
                            ny(pts[id].y),
                            aura_style(aura[id]),
                        ),
                        if leader == Some(id) {
                            Crown {}
                        }
                        span { class: "pg-node-id", "{id}" }
                        if !timers[id].is_empty() {
                            div {
                                class: "pg-node-timers",
                                style: {
                                    let (dx, dy) = (base_pts[id].x - 0.5, base_pts[id].y - 0.5);
                                    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
                                    let (ux, uy) = (dx / len, dy / len);
                                    let (out, inc) = bar_counts[id];
                                    let off = timer_offset_rem(out, inc, ux, uy);
                                    format!("--tx: {:.3}rem; --ty: {:.3}rem;", ux * off, uy * off)
                                },
                                for (role , cls , arrow , head) in [
                                    (TimerRole::Grantor, "pg-timer-col is-out", "\u{2192}", "OUT"),
                                    (TimerRole::Grantee, "pg-timer-col is-in", "\u{2190}", "IN"),
                                ]
                                {
                                    if timers[id].iter().any(|t| t.role == role) {
                                        div { key: "{head}", class: cls,
                                            span { class: "pg-timer-colhead", "{head}" }
                                            for (j , tb) in timers[id].iter().filter(|t| t.role == role).enumerate() {
                                                div {
                                                    key: "{j}",
                                                    class: "pg-timer-cell",
                                                    "data-hint": format!(
                                                        "as {} node {} · {}",
                                                        match role {
                                                            TimerRole::Grantor => "grantor to",
                                                            TimerRole::Grantee => "grantee of",
                                                        },
                                                        tb.other,
                                                        match tb.status {
                                                            LeaseStatus::Active => "active",
                                                            LeaseStatus::Guarding => "guarding",
                                                            LeaseStatus::Expired => "expired",
                                                            LeaseStatus::Inactive => "idle",
                                                        },
                                                    ),
                                                    span { class: "pg-timer-label",
                                                        "{arrow}"
                                                        "{tb.other}"
                                                    }
                                                    div { class: "pg-timer",
                                                        // Active drains in the role color; Guarding drains in
                                                        // blue (its guard-window countdown); idle/expired have
                                                        // no live countdown, shown as a solid gray track.
                                                        match tb.status {
                                                            LeaseStatus::Active => rsx! {
                                                                div { class: "pg-timer-fill", style: "width: {tb.fill * 100.0}%;" }
                                                            },
                                                            LeaseStatus::Guarding => rsx! {
                                                                div { class: "pg-timer-fill is-guarding", style: "width: {tb.fill * 100.0}%;" }
                                                            },
                                                            _ => rsx! {
                                                                div { class: "pg-timer-fill is-inactive" }
                                                            },
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                svg {
                    class: "pg-edges",
                    view_box: "{view_box}",
                    preserve_aspect_ratio: "none",
                    defs {
                        marker {
                            id: "pg-arrow",
                            view_box: "0 0 10 10",
                            ref_x: "8",
                            ref_y: "5",
                            marker_units: "userSpaceOnUse",
                            marker_width: "0.032",
                            marker_height: "0.032",
                            orient: "auto",
                            path {
                                class: "pg-arrow-head",
                                d: "M0,0 L10,5 L0,10 z",
                            }
                        }
                    }
                    for (i , e) in edges.iter().enumerate() {
                        {
                            let st = stem(*e);
                            rsx! {
                                line {
                                    key: "s{i}",
                                    class: "pg-edge",
                                    x1: "{st.x1}",
                                    y1: "{st.y1}",
                                    x2: "{st.x2}",
                                    y2: "{st.y2}",
                                }
                            }
                        }
                    }
                    for (i , e) in edges.iter().enumerate() {
                        line {
                            key: "h{i}",
                            class: "pg-edge-head",
                            x1: "{e.x1}",
                            y1: "{e.y1}",
                            x2: "{e.x2}",
                            y2: "{e.y2}",
                            "marker-end": "url(#pg-arrow)",
                        }
                    }
                }
                for id in 0..n {
                    div {
                        key: "{id}",
                        class: if grantors.contains(&id) && grantees.contains(&id) { "pg-node is-grantor is-grantee" } else if grantors.contains(&id) { "pg-node is-grantor" } else if grantees.contains(&id) { "pg-node is-grantee" } else { "pg-node" },
                        style: format!("left: {:.3}%; top: {:.3}%;", pts[id].x * 100.0, ny(pts[id].y)),
                        if leader == Some(id) {
                            Crown {}
                        }
                        span { class: "pg-node-id", "{id}" }
                    }
                }
            }
        }
    }
}

/// The run bar (`[control | track | status]` grid) plus the time axis beneath it:
/// the primary Play/Pause/Resume button + clock glyph, the timeline slider, the
/// status cell, then Restart + the time readout + the fast-forward toggle. Shared
/// verbatim by the playground and scenario canvases; the caller supplies the
/// phase, callbacks, and the status cell content.
#[component]
#[allow(clippy::too_many_arguments)]
pub fn RunBar(
    phase: RunPhase,
    runnable: bool,
    rec_len: usize,
    cur: usize,
    now_ticks: Time,
    end_ticks: Time,
    fast_forward: bool,
    on_play: EventHandler<()>,
    on_pause: EventHandler<()>,
    on_resume: EventHandler<()>,
    on_restart: EventHandler<()>,
    on_scrub: EventHandler<usize>,
    on_toggle_ff: EventHandler<()>,
    /// Whether to show the "3x ⏩" fast-forward toggle. The playground shows it;
    /// the walkthrough's scenario canvases hide it (they play at a fixed pace).
    #[props(default = true)]
    show_ff: bool,
    /// When set, the Play button is locked (disabled) once the run has `Stopped`,
    /// so the user must press Restart to run again — a scenario canvas that halts
    /// at its stop condition shouldn't offer a bare replay in place. The
    /// playground leaves this off (Play restarts a fresh run after a stop).
    #[props(default = false)]
    lock_when_stopped: bool,
    status: Element,
) -> Element {
    let generating = phase == RunPhase::Generating;
    let paused = phase == RunPhase::Paused;
    let stopped = phase == RunPhase::Stopped;
    let has_run = phase != RunPhase::Idle;
    let scrubbable = matches!(phase, RunPhase::Paused | RunPhase::Stopped);
    // Play is disabled if the scenario declares no lease, or — for a canvas that
    // locks after stopping — once it has stopped (Restart is the way back).
    let play_disabled = !runnable || (lock_when_stopped && stopped);

    rsx! {
        div { class: "pg-runbar",
            div { class: "pg-runctrl",
                if generating {
                    button {
                        class: "pg-btn is-stop",
                        onclick: move |_| on_pause.call(()),
                        "Pause"
                    }
                } else if paused {
                    button { class: "pg-btn", onclick: move |_| on_resume.call(()), "Resume" }
                } else {
                    button {
                        class: "pg-btn",
                        disabled: play_disabled,
                        onclick: move |_| on_play.call(()),
                        "Play"
                    }
                }
                svg { class: "pg-timeline-icon", view_box: "0 0 24 24",
                    circle { cx: "12", cy: "12", r: "9" }
                    path { d: "M12 7 L12 12 L15.5 14" }
                }
            }
            input {
                class: "pg-timeline-slider",
                r#type: "range",
                min: "0",
                max: "{rec_len.saturating_sub(1)}",
                step: "1",
                value: "{cur}",
                disabled: !scrubbable,
                oninput: move |e| {
                    if let Ok(v) = e.value().parse::<usize>() {
                        on_scrub.call(v);
                    }
                },
            }
            div { class: "pg-runstatus", {status} }
        }
        div { class: "pg-timeaxis",
            button {
                class: "pg-btn is-ghost",
                disabled: !has_run,
                onclick: move |_| on_restart.call(()),
                "Restart"
            }
            span { class: "pg-axis-track",
                span { class: "pg-axis-spacer" }
                span { class: "pg-time-readout",
                    "t = "
                    strong { "{now_ticks}" }
                }
                span { class: "pg-axis-end", "max {end_ticks} ticks" }
            }
            if show_ff {
                button {
                    class: if fast_forward { "pg-ff is-on" } else { "pg-ff" },
                    onclick: move |_| on_toggle_ff.call(()),
                    // Label mirrors FF_MULT (3); keep in sync if that changes.
                    "3x ⏩\u{fe0e}"
                }
            }
        }
    }
}

/// The per-node grant-status bars, on the shared run-bar grid so every bar lines
/// up with the timeline track. `bars` is the run-length-encoded grant history
/// (see [`grant_bars`]); `cursor_frac` positions the playhead.
#[component]
pub fn GrantBars(
    topology: Topology,
    bars: Vec<Vec<GrantRun>>,
    cursor_frac: f64,
    rec_len: usize,
    /// When set, only nodes that are grantees get a row (grants are always held
    /// *as a grantee*, so a non-grantee's row is an always-empty track). The
    /// walkthrough canvases set this; the playground shows every node.
    #[props(default = false)]
    grantees_only: bool,
    /// Whether the timeline slider is currently draggable (the run is paused or
    /// finished). When set, a small "you can drag" hint is pinned to the right of
    /// the bars, cueing that the timeline can be scrubbed.
    #[props(default = false)]
    draggable: bool,
) -> Element {
    let n = topology.n;
    let max_grants = topology.max_grants();
    let rows: Vec<usize> = (0..n)
        .filter(|id| !grantees_only || topology.grantees.contains(id))
        .collect();
    rsx! {
        div { class: "pg-grants",
            span {
                class: "pg-grants-header pg-hint",
                "data-hint": "Per node, how many grants it holds as a grantee over the run — a green segment per interval, darker with more grants held, empty when it holds none",
                "Grants?"
            }
            if draggable {
                span { class: "pg-grants-drag", "you can drag" }
            }
            div { class: "pg-grantbars",
                for id in rows.iter().copied() {
                    div { key: "{id}", class: "pg-grantbar-row",
                        span { class: "pg-grantbar-id", "{id}" }
                        div { class: "pg-grantbar",
                            for (i , run) in bars[id].iter().enumerate() {
                                div {
                                    key: "{i}",
                                    class: "pg-grantbar-seg",
                                    style: "flex-grow: {run.frames}; background-color: {grant_color(run.grants, max_grants[id])};",
                                    "data-hint": format!(
                                        "{} grant{} held · {} ticks",
                                        run.grants,
                                        plural(run.grants),
                                        run.frames as i64 * FRAME_TICKS,
                                    ),
                                }
                            }
                            if rec_len > 0 {
                                div {
                                    class: "pg-grantbar-cursor",
                                    style: "left: {cursor_frac * 100.0}%;",
                                }
                            }
                        }
                        span {}
                    }
                }
            }
        }
    }
}

/// A boxed timing constant `T_name`, math-styled with a subscript and a hover
/// tooltip. Shared by the playground caption and any scenario captions.
#[component]
pub fn TConst(name: &'static str, hint: &'static str, ticks: &'static str) -> Element {
    rsx! {
        span { class: "pg-tconst", "data-hint": "{hint} ({ticks})",
            "T"
            sub { "{name}" }
        }
    }
}
