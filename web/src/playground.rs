//! The simulator playground: scenario-setup controls over a live canvas that
//! reflects the chosen nodes and grantor -> grantee lease relationships.
//!
//! A `Play` press builds a [`Scenario`] from the current knobs and *generates*
//! the run live: the `lease_sim` engine is advanced [`FRAME_TICKS`] ticks per
//! wall-clock step, each step recording a `Frame` and animating on the canvas.
//! The run keeps playing until the user pauses it or it reaches the [`MAX_TICKS`]
//! cap. While running the timeline slider is inert and a spinner shows; once
//! paused (or capped) the whole recorded run stays put and the slider freely
//! scrubs it, and a paused run can be resumed from where it left off. Any change
//! to a scenario knob discards the run and returns the canvas to static editing.

use std::collections::BTreeSet;
use std::time::Duration;

use dioxus::prelude::*;
use lease_sim::frame::{lerp, ring_layout};
use lease_sim::{Engine, Frame, LeaseStatus, MsgFate, MsgKind, NodeId, Point, Scenario, Time};

/// Node-count bounds for the slider.
const MIN_NODES: usize = 2;
const MAX_NODES: usize = 9;

/// Global ticks between recorded frames — the time resolution of the run and
/// the granularity the timeline slider scrubs at. Kept fine for smooth motion.
const FRAME_TICKS: Time = 5;
/// Wall-clock interval between generation repaints (~83 fps). Paired with
/// `FRAMES_PER_STEP = 1` so exactly one recorded frame is painted per repaint —
/// the smoothest possible display at the current `FRAME_TICKS` resolution
/// (repainting faster would only re-show duplicate frames). The recorded frames
/// (and thus the scrubbable run) are unaffected; only the live-generation
/// wall-clock cadence changes.
const RENDER_MS: u32 = 12;
/// Frames advanced per repaint while generating. One frame per repaint keeps
/// display and sim resolution in lockstep; playback speed is
/// `FRAMES_PER_STEP · FRAME_TICKS / RENDER_MS` ticks of sim per real ms, held at
/// `1·5/12 = 5/12` (unchanged from the prior `3·5/36`).
const FRAMES_PER_STEP: usize = 1;
/// Frames-per-repaint multiplier applied while the fast-forward toggle is on:
/// the loop advances `FRAMES_PER_STEP · FF_MULT` frames per repaint, playing the
/// run (and its animation) that many times faster.
const FF_MULT: usize = 3;
/// Cap on how long a run may play. A run keeps generating until the user pauses
/// it or it reaches this many ticks; hitting the cap ends the run as `Capped`.
const MAX_TICKS: Time = 60_000;

/// A drop-rate choice for a message kind: never, rarely, sometimes, or always.
/// The selectable failure levels for the Guard / Renew failure switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailRate {
    Off,
    Tiny,
    Low,
    Some,
    All,
}

impl FailRate {
    /// The drop probability this rate applies to its message kind.
    fn prob(self) -> f64 {
        match self {
            FailRate::Off => 0.0,
            FailRate::Tiny => 0.01,
            FailRate::Low => 0.10,
            FailRate::Some => 0.30,
            FailRate::All => 1.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            FailRate::Off => "Off",
            FailRate::Tiny => "1%",
            FailRate::Low => "10%",
            FailRate::Some => "30%",
            FailRate::All => "100%",
        }
    }
}

/// How often the leader serves a write request: never, or on an average
/// interval (± jitter). The selectable values for the "Every" write switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteEvery {
    Never,
    Slow,
    Mid,
    Fast,
}

impl WriteEvery {
    /// The average write interval in ticks, or `None` to never write.
    fn interval(self) -> Option<Time> {
        match self {
            WriteEvery::Never => None,
            WriteEvery::Slow => Some(3000),
            WriteEvery::Mid => Some(1000),
            WriteEvery::Fast => Some(300),
        }
    }

    fn label(self) -> &'static str {
        match self {
            WriteEvery::Never => "Never",
            WriteEvery::Slow => "3000 ticks",
            WriteEvery::Mid => "1000 ticks",
            WriteEvery::Fast => "300 ticks",
        }
    }
}

/// A starting-point scenario shape: the four algorithm levels plus a small
/// lease-manager topology (one grantor fanning out to several grantees).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Preset {
    OneToOne,
    LeaseManager,
    Leader,
    Quorum,
    Roster,
}

impl Preset {
    /// (node count, grantor ids, grantee ids) this preset expands to.
    fn expand(self) -> (usize, Vec<usize>, Vec<usize>) {
        match self {
            Preset::OneToOne => (2, vec![0], vec![1]),
            Preset::LeaseManager => (4, vec![0], vec![1, 2, 3]),
            Preset::Leader => (5, (0..5).collect(), vec![0]),
            Preset::Quorum => (5, (0..5).collect(), vec![1, 3]),
            Preset::Roster => (5, (0..5).collect(), (0..5).collect()),
        }
    }

    /// The failure/write switch settings this preset presents:
    /// `(guard_fail, renew_fail, write_every, write_disruptive)`. Every preset
    /// uses a 1% Guard/Renew drop; they differ only in write cadence/mode.
    fn switches(self) -> (FailRate, FailRate, WriteEvery, bool) {
        match self {
            Preset::OneToOne => (FailRate::Tiny, FailRate::Tiny, WriteEvery::Never, false),
            Preset::LeaseManager => (FailRate::Tiny, FailRate::Tiny, WriteEvery::Never, false),
            Preset::Leader => (FailRate::Tiny, FailRate::Tiny, WriteEvery::Slow, false),
            Preset::Quorum => (FailRate::Tiny, FailRate::Tiny, WriteEvery::Slow, true),
            Preset::Roster => (FailRate::Tiny, FailRate::Tiny, WriteEvery::Slow, false),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Preset::OneToOne => "One-to-One",
            Preset::LeaseManager => "Lease Manager",
            Preset::Leader => "Leader Leases",
            Preset::Quorum => "Quorum Leases",
            Preset::Roster => "Roster Leases",
        }
    }
}

/// Lifecycle of a playground run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// No run; the canvas shows the static scenario and knobs are editable.
    Idle,
    /// A run is being generated live (the simulation is playing out).
    Generating,
    /// Paused mid-generation via the Pause button. Scrubbable, and resumable —
    /// pressing Resume continues generating from where it left off.
    Paused,
    /// Finished at the [`MAX_TICKS`] cap. The slider now scrubs the run.
    Capped,
}

/// Bookkeeping carried across generation steps.
#[derive(Clone, PartialEq)]
struct GenState {
    /// Global time of the next frame to generate.
    t: Time,
}

/// Strict majority (quorum-intersection threshold) for a cluster of size `n`.
fn majority(n: usize) -> usize {
    n / 2 + 1
}

/// A fresh random seed for a run, drawn from the browser's `Math.random()`. The
/// engine is deterministic *given* a seed; picking a new one per Play is what
/// makes each run's stochastic drops/timings differ. The two random draws fill
/// the low and high 32 bits so the whole `u64` seed space is reachable.
fn fresh_seed() -> u64 {
    let hi = (js_sys::Math::random() * (1u64 << 32) as f64) as u64;
    let lo = (js_sys::Math::random() * (1u64 << 32) as f64) as u64;
    (hi << 32) | lo
}

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
/// coordinates: `(ref_x / viewBox_width) · marker_width` = `(8/10)·0.032`. The
/// visible stem is pulled back by this much so it stops at the arrowhead's base
/// and never shows through the (semi-transparent) head.
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

/// A lease edge to draw during playback, colored by the grantee's held status.
#[derive(Debug, Clone, Copy)]
struct LeaseEdge {
    e: Edge,
    status: LeaseStatus,
    /// Grantee's remaining lease life, `0.0..1.0`, for a countdown fade.
    fill: f64,
}

/// One run-length run of a node's grant history: the node held `grants` active
/// grants as a grantee for `frames` consecutive recorded frames. Built up
/// append-only as the simulation generates, so completed runs never change. A
/// new run begins whenever the grant count changes.
#[derive(Debug, Clone, Copy, PartialEq)]
struct GrantRun {
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

/// Run-length encode a node's active-grant count over the recorded `frames`.
/// `base` is the node's implicit self-grant (1 if it grants to itself, i.e. it
/// is both a grantor and a grantee — such a node always holds that 1), added on
/// top of the grants it holds from others. Derived from `frames` (the single
/// source of truth that persists once a run stops), so bars stay put after
/// generation ends.
fn grant_runs(frames: &[Frame], node: NodeId, base: usize) -> Vec<GrantRun> {
    // Before any run exists, still show the node's standing self-grant (`base`)
    // as a single full-width segment, so a self-grantor's bar is colored the
    // moment its scenario is selected.
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

/// Plural suffix for a count: "" for 1, "s" otherwise.
fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// CSS color for a grant-status run, as a plain comma-free hex string: an empty
/// track (`--surface`) when no grant is held, else grantee green mixed toward
/// white, deepening with how many of the node's possible grants (`max`) are held.
///
/// Returns hex (not `color-mix(...)`/`var(...)`) on purpose: Dioxus's inline
/// `style` string parser drops values containing commas, so a `color-mix` or
/// gradient silently renders as no style at all.
fn grant_color(grants: usize, max: usize) -> String {
    if grants == 0 || max == 0 {
        return "#f4f5f7".to_string(); // matches --surface (empty track)
    }
    let hex = |c: (f64, f64, f64), amt: f64, toward: f64| {
        let mix = |x: f64| (x * amt + toward * (1.0 - amt)).round() as u8;
        format!("#{:02x}{:02x}{:02x}", mix(c.0), mix(c.1), mix(c.2))
    };
    // Grantee green (#2f8f5b) mixed toward white; deeper with more grants.
    const G: (f64, f64, f64) = (0x2f as f64, 0x8f as f64, 0x5b as f64);
    let frac = (grants as f64 / max as f64).clamp(0.0, 1.0);
    let green = 0.30 + 0.55 * frac; // 30%..85% green, remainder white
    hex(G, green, 255.0)
}

/// Inline style for a node's green "aura" halo, sized and shaded by `frac` (the
/// fraction of its possible grants the grantee currently holds, `0.0..1.0`) —
/// the same ratio the grant bars shade green by. Sets two comma-free CSS custom
/// properties the `.pg-node` `box-shadow` reads (`--aura-blur`/`--aura-spread`
/// grow the halo, `--aura-color` deepens it); an empty string when `frac <= 0`
/// so a node holding nothing shows no halo.
///
/// Comma-free on purpose: Dioxus's inline `style` parser drops any value
/// containing a comma, so the multi-layer `box-shadow` itself lives in the
/// stylesheet and only these scalar vars are set inline.
fn aura_style(frac: f64) -> String {
    if frac <= 0.0 {
        return String::new();
    }
    let frac = frac.clamp(0.0, 1.0);
    let spread = 2.0 + 2.5 * frac; // 2.0px..5.0px ring (old fixed halo was 3px)
    let blur = 1.0 + 2.0 * frac; // 1px..3px soft glow
    // Grantee green (#2f8f5b) mixed toward the surface (#f4f5f7), deeper green
    // with more grants — mirroring `grant_color`.
    const G: (f64, f64, f64) = (0x2f as f64, 0x8f as f64, 0x5b as f64);
    const S: (f64, f64, f64) = (0xf4 as f64, 0xf5 as f64, 0xf7 as f64);
    let green = 0.35 + 0.5 * frac; // 35%..85% green, remainder surface
    let mix = |c: f64, s: f64| (c * green + s * (1.0 - green)).round() as u8;
    format!(
        "--aura-blur: {blur:.2}px; --aura-spread: {spread:.2}px; --aura-color: #{:02x}{:02x}{:02x};",
        mix(G.0, S.0),
        mix(G.1, S.1),
        mix(G.2, S.2),
    )
}

/// Build a scenario from the chosen knobs: every node initiates the leases it
/// grants (per-poll chance, so guarding starts at a staggered random time),
/// links have no baseline loss, and no node fails. The Guard / Renew failure
/// switches inject a per-kind drop probability on top, and the write switches
/// set the leader's write cadence and disruptiveness.
#[allow(clippy::too_many_arguments)]
fn build_scenario(
    n: usize,
    grantors: &BTreeSet<usize>,
    grantees: &BTreeSet<usize>,
    guard_fail: FailRate,
    renew_fail: FailRate,
    write_every: WriteEvery,
    write_disruptive: bool,
    seed: u64,
) -> Scenario {
    let mut s = Scenario::new(n)
        .seed(seed)
        .duration(MAX_TICKS)
        .all_nodes(|nc| nc.initiate_chance = 0.5)
        .kind_drop(MsgKind::Guard, guard_fail.prob())
        .kind_drop(MsgKind::Renew, renew_fail.prob())
        .writes(write_every.interval(), write_disruptive);
    for &g in grantors {
        for &h in grantees {
            if g != h && g < n && h < n {
                s = s.lease(g, h);
            }
        }
    }
    s
}

/// Whether the chosen knobs declare at least one grantor -> grantee lease.
fn has_leases(n: usize, grantors: &BTreeSet<usize>, grantees: &BTreeSet<usize>) -> bool {
    grantors
        .iter()
        .any(|&g| grantees.iter().any(|&h| g != h && g < n && h < n))
}

fn toggle_id(mut set: Signal<BTreeSet<usize>>, id: usize) {
    let mut s = set.write();
    if !s.remove(&id) {
        s.insert(id);
    }
}

/// Select all `0..n` if not already all-selected; otherwise clear.
fn toggle_all(mut set: Signal<BTreeSet<usize>>, n: usize) {
    let all: BTreeSet<usize> = (0..n).collect();
    if *set.read() == all {
        set.set(BTreeSet::new());
    } else {
        set.set(all);
    }
}

/// Arrowhead marker id for a lease edge in playback, colored to match its
/// stem: green when active/renewing, light blue while guarding, gray when idle.
fn ledge_marker(status: LeaseStatus) -> &'static str {
    match status {
        LeaseStatus::Active => "url(#pg-arrow-active)",
        LeaseStatus::Guarding => "url(#pg-arrow-guarding)",
        _ => "url(#pg-arrow-idle)",
    }
}

/// CSS class for a message glyph, grouped (colored) by protocol phase (guard
/// blue, renew green, revoke orange, write purple, commit dark gray). Reply
/// kinds add `is-reply`, which lightens the phase color a touch.
fn msg_class(kind: MsgKind) -> &'static str {
    match kind {
        MsgKind::Guard => "pg-msg is-guard",
        MsgKind::GuardReply => "pg-msg is-guard is-reply",
        MsgKind::Renew => "pg-msg is-renew",
        MsgKind::RenewReply => "pg-msg is-renew is-reply",
        MsgKind::Revoke => "pg-msg is-revoke",
        MsgKind::Write => "pg-msg is-write",
        MsgKind::WriteReply => "pg-msg is-write is-reply",
        MsgKind::Commit => "pg-msg is-commit",
    }
}

/// Whether a message is an acknowledgement reply — drawn with a small
/// "thumbs-up" badge over its base glyph.
fn msg_is_reply(kind: MsgKind) -> bool {
    matches!(
        kind,
        MsgKind::GuardReply | MsgKind::RenewReply | MsgKind::WriteReply
    )
}

/// Opacity of an in-flight message glyph at flight `progress` (`0.0..1.0` from
/// sender to receiver): fully opaque through the middle of the flight, fading
/// *in* over the initial departure and *out* over the final approach — so it
/// emerges from the sender node and vanishes right as it reaches the
/// destination node (progress 1.0), rather than popping in/out at the borders.
fn msg_opacity(progress: f64) -> f64 {
    const FADE: f64 = 0.25; // fade over the first / last quarter of the flight
    if progress < FADE {
        (progress / FADE).clamp(0.0, 1.0)
    } else if progress > 1.0 - FADE {
        ((1.0 - progress) / FADE).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// Flight `progress` at which a *dropped* message dies mid-link: it travels this
/// far from the sender, then vanishes in a red burst instead of reaching the
/// destination. Kept short of the midpoint so the drop reads as happening in
/// transit, not at the far node.
const DROP_AT: f64 = 0.45;
/// How much flight `progress` the red drop burst plays out over, starting at
/// [`DROP_AT`]. The dropped message stays "in flight" (per the engine) until its
/// would-be arrival, so there is always room for the burst after it.
const BURST_DUR: f64 = 0.4;

/// Opacity of a *dropped* message's glyph at flight `progress`: fades in on
/// departure like a normal message, then fades out sharply as it reaches the
/// drop point ([`DROP_AT`]), handing off to the burst. Zero once dropped.
fn drop_glyph_opacity(progress: f64) -> f64 {
    const FADE_IN: f64 = 0.15;
    const FADE_OUT: f64 = 0.1; // last stretch before DROP_AT
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

/// Burst animation parameter for a dropped message at flight `progress`, as
/// `Some(bp)` with `bp` in `0.0..1.0` over the burst window, or `None` when the
/// burst is not playing (before the drop or after it has faded).
fn drop_burst(progress: f64) -> Option<f64> {
    if progress < DROP_AT {
        return None;
    }
    let bp = (progress - DROP_AT) / BURST_DUR;
    (bp <= 1.0).then_some(bp.clamp(0.0, 1.0))
}

/// Symbol for an in-flight message, drawn inside its positioned `.pg-msg` chip:
/// a shield for guard-phase messages, a circular "renew" arrow for renewals, a
/// dot for revokes, a pencil for writes, a checkmark for commits. Reply kinds
/// (`*Reply`) overlay a small thumbs-up badge to mark them as acknowledgements.
/// Colored by phase via `currentColor` (set on `.pg-msg` in CSS).
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
                // Rotated 180° about the icon center so the head/tail gap sits
                // at the lower left rather than the upper right.
                path {
                    transform: "rotate(180 12 12)",
                    d: "M17.65 6.35C16.2 4.9 14.21 4 12 4c-4.42 0-7.99 3.58-7.99 8s3.57 8 7.99 8c3.73 0 6.84-2.55 7.73-6h-2.08c-.82 2.33-3.04 4-5.65 4-3.31 0-6-2.69-6-6s2.69-6 6-6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z",
                }
            }
        },
        MsgKind::Revoke => rsx! {
            span { class: "pg-msg-dot" }
        },
        // Write / WriteReply → a pencil (edit) glyph.
        MsgKind::Write | MsgKind::WriteReply => rsx! {
            svg { class: "pg-msg-icon", view_box: "0 0 24 24",
                path { d: "M3 17.25V21h3.75L17.81 9.94l-3.75-3.75L3 17.25zM20.71 7.04c.39-.39.39-1.02 0-1.41l-2.34-2.34a.9959.9959 0 0 0-1.41 0l-1.83 1.83 3.75 3.75 1.83-1.83z" }
            }
        },
        // Commit → a bold checkmark.
        MsgKind::Commit => rsx! {
            svg { class: "pg-msg-icon", view_box: "0 0 24 24",
                path { d: "M9 16.17 L4.83 12 l-1.42 1.41 L9 19 L21 7 l-1.41-1.41 z" }
            }
        },
    };
    rsx! {
        {base}
        if msg_is_reply(kind) {
            // Thumbs-up badge, tucked at the top-right like a superscript.
            span { class: "pg-msg-badge",
                svg { class: "pg-msg-badge-icon", view_box: "0 0 24 24",
                    path { d: "M1 21h4V9H1v12zm22-11c0-1.1-.9-2-2-2h-6.31l.95-4.57.03-.32c0-.41-.17-.79-.44-1.06L14.17 1 7.59 7.59C7.22 7.95 7 8.45 7 9v10c0 1.1.9 2 2 2h9c.83 0 1.54-.5 1.84-1.22l3.02-7.05c.09-.23.14-.47.14-.73v-2z" }
                }
            }
        }
    }
}

/// A small crown badge marking the "leader" node (the smallest-id grantee),
/// tucked just above the node disk. Purely a topology annotation.
#[component]
fn Crown() -> Element {
    rsx! {
        svg { class: "pg-crown", view_box: "0 0 24 24",
            // Five-point crown: base bar with three spikes and two dips.
            path { d: "M3 8 L6.5 13 L12 6 L17.5 13 L21 8 L19 19 L5 19 Z" }
        }
    }
}

#[component]
pub fn Playground() -> Element {
    let mut node_count = use_signal(|| 2usize);
    let mut grantors = use_signal(|| BTreeSet::from([0usize]));
    let mut grantees = use_signal(|| BTreeSet::from([1usize]));
    let mut preset = use_signal(|| Some(Preset::OneToOne));
    // Per-kind message-failure switches; not part of the scenario *shape*, so
    // they don't clear the chosen preset when changed. Applying a preset sets
    // these too; the initial values match the default One-to-One preset.
    let mut guard_fail = use_signal(|| FailRate::Tiny);
    let mut renew_fail = use_signal(|| FailRate::Tiny);
    // Write-load switches: how often the leader serves a write, and whether that
    // write is disruptive. Like the failure switches, not part of the shape.
    let mut write_every = use_signal(|| WriteEvery::Never);
    let mut write_disruptive = use_signal(|| false);

    // Run state: lifecycle phase, the live engine (only during generation), the
    // recorded frames, per-run bookkeeping, and the scrub cursor.
    let mut phase = use_signal(|| Phase::Idle);
    let mut engine = use_signal(|| None::<Engine>);
    let mut frames = use_signal(Vec::<Frame>::new);
    let mut gen_state = use_signal(|| None::<GenState>);
    let mut cursor = use_signal(|| 0usize);
    // Fast-forward toggle: when on, the generation loop advances `FF_MULT` frames
    // per repaint instead of one, so a run plays out (and animates) that much
    // faster. A playback preference, not part of the scenario shape.
    let mut fast_forward = use_signal(|| false);

    // Discard any run, returning the canvas to static editing.
    let mut reset_sim = move || {
        phase.set(Phase::Idle);
        engine.set(None);
        frames.write().clear();
        gen_state.set(None);
        cursor.set(0);
    };

    // Generation loop: advance the engine live while a run is being generated,
    // a batch of `FRAMES_PER_STEP` fine-grained frames per repaint. Advancing
    // live (rather than all at once) is what makes the run visible; the batch
    // keeps wall-clock pace reasonable at a fine time resolution. The run plays
    // on until the user pauses it (phase leaves `Generating`, keeping the engine
    // for a later resume) or reaches the `MAX_TICKS` cap, which ends it as
    // `Capped` and drops the engine, leaving the recorded frames for scrubbing.
    use_future(move || async move {
        loop {
            gloo_timers::future::sleep(Duration::from_millis(RENDER_MS as u64)).await;
            if *phase.peek() != Phase::Generating {
                continue;
            }
            let mut last_idx = None;
            let mut capped = false;
            // Fast-forward advances more frames per repaint (same frame
            // resolution, faster wall-clock playback).
            let batch = FRAMES_PER_STEP * if *fast_forward.peek() { FF_MULT } else { 1 };
            for _ in 0..batch {
                let t = match gen_state.peek().as_ref() {
                    Some(gs) => gs.t,
                    None => break,
                };
                // Advance the engine to `t` and snapshot its frame.
                let frame = {
                    let mut eng = engine.write();
                    match eng.as_mut() {
                        Some(e) => {
                            e.advance_to(t);
                            e.frame_at(t)
                        }
                        None => break,
                    }
                };
                // Advance the frame clock, or flag the cap once reached.
                if t >= MAX_TICKS {
                    capped = true;
                } else {
                    gen_state.write().as_mut().unwrap().t = t + FRAME_TICKS;
                }
                // Append the frame; track the newest index for the live view.
                let mut fr = frames.write();
                fr.push(frame);
                last_idx = Some(fr.len() - 1);
                if capped {
                    break;
                }
            }
            // Keep the live view on the newest frame of this batch.
            if let Some(idx) = last_idx {
                cursor.set(idx);
            }
            if capped {
                phase.set(Phase::Capped);
                engine.set(None); // recorded frames remain; free the engine
            }
        }
    });

    // Apply a preset: set the count, both selection sets, and the failure/write
    // switches in one shot.
    let mut apply_preset = move |p: Preset| {
        let (n, gs, hs) = p.expand();
        node_count.set(n);
        grantors.set(gs.into_iter().collect());
        grantees.set(hs.into_iter().collect());
        let (gf, rf, we, wd) = p.switches();
        guard_fail.set(gf);
        renew_fail.set(rf);
        write_every.set(we);
        write_disruptive.set(wd);
        preset.set(Some(p));
        reset_sim();
    };

    // Shrinking the cluster drops any selected ids that no longer exist.
    let mut set_node_count = move |n: usize| {
        node_count.set(n);
        grantors.write().retain(|&id| id < n);
        grantees.write().retain(|&id| id < n);
        preset.set(None);
        reset_sim();
    };

    let n = node_count();
    let maj = majority(n);
    let pts = ring_layout(n);
    let runnable = has_leases(n, &grantors.read(), &grantees.read());
    // The "leader" is the smallest-id grantee (an all-to-one leader is the sole
    // grantee; more generally we just crown the lowest-id one). Marked with a
    // crown in the topology, in both the static and playback views.
    let leader: Option<usize> = grantees.read().iter().copied().find(|&id| id < n);

    // Snapshot run state for this render.
    let ph = phase();
    let rec_len = frames.read().len();
    let cur = cursor().min(rec_len.saturating_sub(1));
    let current_frame: Option<Frame> = frames.read().get(cur).cloned();
    // Cursor position as a 0..1 fraction of the run, for the playhead markers
    // drawn on the grant bars in sync with the timeline slider handle.
    let cursor_frac = if rec_len > 1 {
        cur as f64 / (rec_len - 1) as f64
    } else {
        0.0
    };

    // Play: (re)build the scenario and begin a fresh live generation.
    let on_start = move |_| {
        if !runnable {
            return;
        }
        let n = node_count();
        let gs = grantors.read().clone();
        let hs = grantees.read().clone();
        let scenario = build_scenario(
            n,
            &gs,
            &hs,
            guard_fail(),
            renew_fail(),
            write_every(),
            write_disruptive(),
            fresh_seed(),
        );
        engine.set(Some(Engine::new(scenario)));
        frames.write().clear();
        cursor.set(0);
        gen_state.set(Some(GenState { t: 0 }));
        phase.set(Phase::Generating);
    };

    // Pause: halt an in-progress generation at the current frame, keeping the
    // engine and bookkeeping so Resume can pick the run back up. The recorded
    // frames stay scrubbable while paused.
    let on_pause = move |_| {
        if *phase.peek() == Phase::Generating {
            phase.set(Phase::Paused);
        }
    };

    // Resume: continue generating a paused run from where it left off. The engine
    // and `gen_state` were kept on pause, so the loop resumes at the next frame.
    let on_resume = move |_| {
        if *phase.peek() == Phase::Paused {
            phase.set(Phase::Generating);
        }
    };

    // Restart: discard the current run entirely and return to static editing, so
    // the next Play starts fresh. Enabled whenever a run exists (any non-idle
    // phase), whether it is playing, paused, or capped.
    let on_restart = move |_| reset_sim();

    // The primary run button toggles Play / Pause / Resume by phase.
    let generating = ph == Phase::Generating;
    let paused = ph == Phase::Paused;
    // A run exists (playing, paused, or capped) — Restart is live, timeline scrubs.
    let has_run = ph != Phase::Idle;
    let scrubbable = matches!(ph, Phase::Paused | Phase::Capped);

    // Timeline extents and readouts, in global ticks.
    let end_ticks = (rec_len.saturating_sub(1)) as Time * FRAME_TICKS;
    let now_ticks = cur as Time * FRAME_TICKS;

    // Static grantor → grantee arrows (gray). Drawn while editing, and also as a
    // gray topology backdrop during playback beneath the live lease edges.
    let edges: Vec<Edge> = {
        let gset = grantors.read();
        let hset = grantees.read();
        let mut v = Vec::new();
        for &g in gset.iter() {
            for &h in hset.iter() {
                if g == h || g >= n || h >= n {
                    continue;
                }
                v.push(edge_between(pts[g], pts[h]));
            }
        }
        v
    };

    // Playback derived geometry: lease edges + per-node majority for the glow.
    let lease_edges: Vec<LeaseEdge> = current_frame
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
            current_frame.as_ref().map_or(0, |f| {
                f.leases
                    .iter()
                    .filter(|b| b.grantee == node && b.grantee_status == LeaseStatus::Active)
                    .count()
                    + 1
            })
        })
        .collect();

    // Grant-status bars derive from the recorded frames (the single source of
    // truth, which persists once a run stops). A node that is both a grantor
    // and a grantee grants to itself, so it always holds that 1 implicit grant
    // (`self_grant`). The max a node can hold as a grantee is the grantors that
    // grant to it, plus its own self-grant; a non-grantee has max 0 and renders
    // as an empty track.
    let self_grant: Vec<usize> = (0..n)
        .map(|node| usize::from(grantors.read().contains(&node) && grantees.read().contains(&node)))
        .collect();
    let max_grants: Vec<usize> = (0..n)
        .map(|node| {
            if grantees.read().contains(&node) {
                grantors.read().iter().filter(|&&g| g != node).count() + self_grant[node]
            } else {
                0
            }
        })
        .collect();
    let bars: Vec<Vec<GrantRun>> = {
        let fr = frames.read();
        (0..n)
            .map(|node| grant_runs(&fr, node, self_grant[node]))
            .collect()
    };

    // Per-node aura strength for playback: the fraction of its possible grants
    // (`max_grants`) the node currently holds as a grantee, `0.0..1.0` — the same
    // ratio the grant bars shade their green by. Drives the green halo's size and
    // intensity so a node holding more grants glows larger and deeper. 0 (no
    // grants / not a grantee) means no aura.
    let aura: Vec<f64> = (0..n)
        .map(|node| {
            if max_grants[node] == 0 {
                return 0.0;
            }
            let held_as_grantee =
                current_frame.as_ref().map_or(0, |f| grant_count(f, node)) + self_grant[node];
            (held_as_grantee as f64 / max_grants[node] as f64).clamp(0.0, 1.0)
        })
        .collect();

    rsx! {
        div { class: "pg-root",
            // Row 1: presets.
            div { class: "pg-row",
                span { class: "pg-label", "Preset" }
                div { class: "pg-pills",
                    for p in [
                        Preset::OneToOne,
                        Preset::LeaseManager,
                        Preset::Leader,
                        Preset::Quorum,
                        Preset::Roster,
                    ] {
                        button {
                            key: "{p.label()}",
                            class: if preset() == Some(p) { "pg-pill is-on" } else { "pg-pill" },
                            onclick: move |_| apply_preset(p),
                            "{p.label()}"
                        }
                    }
                }
            }

            // Row 2: node-count slider with majority readout.
            div { class: "pg-row",
                span { class: "pg-label", "Nodes" }
                input {
                    class: "pg-slider",
                    r#type: "range",
                    min: "{MIN_NODES}",
                    max: "{MAX_NODES}",
                    step: "1",
                    value: "{n}",
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<usize>() {
                            set_node_count(v.clamp(MIN_NODES, MAX_NODES));
                        }
                    },
                }
                span { class: "pg-readout",
                    strong { "{n}" }
                    " nodes · majority = "
                    strong { "{maj}" }
                    " (incl. self)"
                }
            }

            // Row 3: grantor selection.
            SelectBar {
                label: "Grantors",
                n,
                selected: grantors,
                on_toggle: move |id| {
                    toggle_id(grantors, id);
                    preset.set(None);
                    reset_sim();
                },
                on_all: move |_| {
                    toggle_all(grantors, n);
                    preset.set(None);
                    reset_sim();
                },
            }

            // Row 4: grantee selection.
            SelectBar {
                label: "Grantees",
                n,
                selected: grantees,
                on_toggle: move |id| {
                    toggle_id(grantees, id);
                    preset.set(None);
                    reset_sim();
                },
                on_all: move |_| {
                    toggle_all(grantees, n);
                    preset.set(None);
                    reset_sim();
                },
            }

            // Row 5: per-kind message-failure switches. These drop a fraction of
            // Guard / Renew messages; changing one discards any run but leaves
            // the scenario shape (and its preset) intact.
            div { class: "pg-row",
                span { class: "pg-label", "Msg drop" }
                div { class: "pg-fails",
                    FailSwitch {
                        label: "Guard",
                        selected: guard_fail(),
                        on_select: move |r| {
                            guard_fail.set(r);
                            reset_sim();
                        },
                    }
                    FailSwitch {
                        label: "Renew",
                        selected: renew_fail(),
                        on_select: move |r| {
                            renew_fail.set(r);
                            reset_sim();
                        },
                    }
                }
            }

            // Row 6: write-load switches — how often the leader serves a write,
            // and whether that write is disruptive. Like the failure switches,
            // changing one discards the run but keeps the scenario shape.
            div { class: "pg-row",
                span { class: "pg-label", "Writes" }
                div { class: "pg-fails",
                    WriteEverySwitch {
                        selected: write_every(),
                        on_select: move |w| {
                            write_every.set(w);
                            reset_sim();
                        },
                    }
                    DisruptiveSwitch {
                        selected: write_disruptive(),
                        on_select: move |d| {
                            write_disruptive.set(d);
                            reset_sim();
                        },
                    }
                }
            }

            // The canvas: static scenario while editing, animated frame otherwise.
            div { class: "pg-stage",
                if current_frame.is_some() {
                    svg {
                        class: "pg-edges",
                        view_box: "0 0 1 1",
                        preserve_aspect_ratio: "none",
                        // One arrowhead marker per status, so a playback edge's
                        // head matches its stem color (green active/renew, light
                        // blue guarding, gray idle).
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
                            // Light-gray head for the static topology backdrop.
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
                        // Static grantor → grantee topology arrows as a gray
                        // backdrop, so the scenario's links are visible from the
                        // start of a run — before any guard link establishes —
                        // with the live lease edges overlaid on top.
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
                        // Stems (pulled back to the arrowhead base), then heads
                        // on top — so no stem shows through a translucent head.
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
                                        opacity: match le.status {
                                            LeaseStatus::Active => 0.4 + 0.55 * le.fill,
                                            LeaseStatus::Guarding => 0.5,
                                            _ => 0.1,
                                        },
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
                                opacity: match le.status {
                                    LeaseStatus::Active => 0.4 + 0.55 * le.fill,
                                    LeaseStatus::Guarding => 0.5,
                                    _ => 0.1,
                                },
                                "marker-end": ledge_marker(le.status),
                            }
                        }
                    }
                    if let Some(f) = current_frame.as_ref() {
                        for (i , m) in f.messages.iter().enumerate() {
                            // The glyph: a delivered message fades out at the
                            // destination; a dropped one fades out early at the
                            // mid-link drop point, handing off to the red burst.
                            {
                                let opacity = match m.fate {
                                    MsgFate::Dropped => drop_glyph_opacity(m.progress),
                                    MsgFate::Delivered => msg_opacity(m.progress),
                                };
                                rsx! {
                                    div {
                                        key: "m{i}",
                                        class: msg_class(m.kind),
                                        style: format!(
                                            "left: {:.3}%; top: {:.3}%; opacity: {:.3};",
                                            m.pos.x * 100.0,
                                            m.pos.y * 100.0,
                                            opacity,
                                        ),
                                        MsgGlyph { kind: m.kind }
                                    }
                                }
                            }
                            // Drop burst: a red shockwave + ✕ at the drop point,
                            // driven by flight `progress` so it stays in sync
                            // while scrubbing the timeline.
                            if m.fate == MsgFate::Dropped {
                                if let Some(bp) = drop_burst(m.progress) {
                                    {
                                        let p = lerp(pts[m.from], pts[m.to], DROP_AT);
                                        rsx! {
                                            div {
                                                key: "d{i}",
                                                class: "pg-drop",
                                                style: format!(
                                                    "left: {:.3}%; top: {:.3}%; --bp: {:.3};",
                                                    p.x * 100.0,
                                                    p.y * 100.0,
                                                    bp,
                                                ),
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
                                if grantors.read().contains(&id) {
                                    c.push_str(" is-grantor");
                                }
                                if grantees.read().contains(&id) {
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
                                pts[id].y * 100.0,
                                aura_style(aura[id]),
                            ),
                            if leader == Some(id) {
                                Crown {}
                            }
                            span { class: "pg-node-id", "{id}" }
                        }
                    }
                } else {
                    svg {
                        class: "pg-edges",
                        view_box: "0 0 1 1",
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
                        // Stems (pulled back to the arrowhead base), then heads
                        // on top — so no stem shows through a translucent head.
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
                            class: if grantors.read().contains(&id) && grantees.read().contains(&id) { "pg-node is-grantor is-grantee" } else if grantors.read().contains(&id) { "pg-node is-grantor" } else if grantees.read().contains(&id) { "pg-node is-grantee" } else { "pg-node" },
                            style: format!("left: {:.3}%; top: {:.3}%;", pts[id].x * 100.0, pts[id].y * 100.0),
                            if leader == Some(id) {
                                Crown {}
                            }
                            span { class: "pg-node-id", "{id}" }
                        }
                    }
                }
            }

            // Run bar: a 3-column grid — [control | track | status] — shared by
            // the axis and grant-bar rows below so slider, axis, and every bar
            // line up on the same track. Control cell = run button + clock glyph.
            div { class: "pg-runbar",
                div { class: "pg-runctrl",
                    // Primary button cycles Play (idle/capped) → Pause (while
                    // running) → Resume (while paused).
                    if generating {
                        button {
                            class: "pg-btn is-stop",
                            onclick: on_pause,
                            "Pause"
                        }
                    } else if paused {
                        button {
                            class: "pg-btn",
                            onclick: on_resume,
                            "Resume"
                        }
                    } else {
                        button {
                            class: "pg-btn",
                            disabled: !runnable,
                            onclick: on_start,
                            "Play"
                        }
                    }
                    // Clock glyph marks the slider as a time control; sits at the
                    // right of the control cell, hugging the slider's left edge.
                    svg { class: "pg-timeline-icon", view_box: "0 0 24 24",
                        circle { cx: "12", cy: "12", r: "9" }
                        path { d: "M12 7 L12 12 L15.5 14" }
                    }
                }
                // Timeline slider: the shared track column; its handle is a
                // vertical "tick" strike rather than a round knob.
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
                            cursor.set(v);
                        }
                    },
                }
                div { class: "pg-runstatus",
                    {
                        match ph {
                            Phase::Generating => rsx! {
                                span { class: "pg-spinner" }
                                span { class: "pg-status", "sim running" }
                            },
                            Phase::Paused => rsx! {
                                span { class: "pg-status", "run stopped" }
                            },
                            Phase::Capped => rsx! {
                                span { class: "pg-status is-error", "✗ ticks limit" }
                            },
                            Phase::Idle => rsx! {
                                span { class: "pg-status pg-status-hint",
                                    if runnable {
                                        "press Play"
                                    } else {
                                        "select a grantor & grantee"
                                    }
                                }
                            },
                        }
                    }
                }
            }

            // Timing axis on the shared grid: the Restart button sits in the
            // control cell (directly under the run button, no wasted row), then
            // the track (scrub time centered, run end at the right), then an
            // empty status cell — all lined up under the slider.
            div { class: "pg-timeaxis",
                button {
                    class: "pg-btn is-ghost",
                    disabled: !has_run,
                    onclick: on_restart,
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
                // Fast-forward toggle in the status cell, under the run status.
                button {
                    class: if fast_forward() { "pg-ff is-on" } else { "pg-ff" },
                    onclick: move |_| fast_forward.toggle(),
                    "3x ⏩\u{fe0e}"
                }
            }

            // Grant-status bars: one per node, on the same shared grid so every
            // bar lines up exactly with the timeline track. The node id sits in
            // the control cell; the bar fills the track cell. Each bar is a
            // run-length series of segments — a new segment begins whenever the
            // active-grant count changes — shaded green (darker with more grants
            // held). Each segment tooltips its interval length and grant count.
            // A "Grants?" header sits vertically centered to the left of the group.
            div { class: "pg-grants",
                span { class: "pg-grants-header", "Grants?" }
                div { class: "pg-grantbars",
                    for id in 0..n {
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
                                // Playhead marker, kept in sync with the timeline slider handle.
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

            p { class: "pg-caption",
                "Hardcoded: "
                TConst { name: "expire", hint: "Lease expiration timeout", ticks: "1500 ticks" }
                " ≈ "
                TConst { name: "guard", hint: "Guard phase timeout", ticks: "1500 ticks" }
                " ≈ 2.5 x "
                TConst { name: "renew", hint: "Lease renewal interval", ticks: "600 ticks" }
                span { class: "pg-sep", ";" }
                TConst { name: "renew", hint: "Lease renewal interval", ticks: "600 ticks" }
                " ≈ 3 x "
                TConst { name: "msg", hint: "Average message delivery delay", ticks: "~200 ticks avg" }
                span { class: "pg-sep", ";" }
                TConst { name: "msg", hint: "Average message delivery delay", ticks: "~200 ticks avg" }
                " ≈ 2 x "
                TConst { name: "Δ", hint: "Bounded clock drift", ticks: "100 ticks" }
                " with ±40% jitter"
                span { class: "pg-sep", ";" }
                TConst { name: "Δ", hint: "Bounded clock drift", ticks: "100 ticks" }
                " is 100 ticks."
            }
        }
    }
}

/// A boxed timing constant `T_name`, math-styled with a subscript and a hover
/// tooltip explaining what it is. `name` is the subscript text (e.g. "expire");
/// `ticks` is the constant's length (e.g. "1500 ticks"), appended to the hint.
#[component]
fn TConst(name: &'static str, hint: &'static str, ticks: &'static str) -> Element {
    rsx! {
        span { class: "pg-tconst", "data-hint": "{hint} ({ticks})",
            "T"
            sub { "{name}" }
        }
    }
}

/// One failure switch: a small label (the message kind) followed by the five
/// mutually-exclusive rate pills (Off / 1% / 10% / 30% / 100%).
#[component]
fn FailSwitch(
    label: &'static str,
    selected: FailRate,
    on_select: EventHandler<FailRate>,
) -> Element {
    rsx! {
        div { class: "pg-fail",
            span { class: "pg-fail-label", "{label}" }
            div { class: "pg-fail-pills",
                for r in [
                    FailRate::Off,
                    FailRate::Tiny,
                    FailRate::Low,
                    FailRate::Some,
                    FailRate::All,
                ] {
                    button {
                        key: "{r.label()}",
                        class: if selected == r { "pg-fail-pill is-on" } else { "pg-fail-pill" },
                        onclick: move |_| on_select.call(r),
                        "{r.label()}"
                    }
                }
            }
        }
    }
}

/// The "Every" write switch: how often the leader serves a write
/// (Never / 3000 ticks / 1000 ticks / 300 ticks). Reuses the failure-switch pill styling.
#[component]
fn WriteEverySwitch(selected: WriteEvery, on_select: EventHandler<WriteEvery>) -> Element {
    rsx! {
        div { class: "pg-fail",
            span { class: "pg-fail-label", "Every" }
            div { class: "pg-fail-pills",
                for w in [
                    WriteEvery::Never,
                    WriteEvery::Slow,
                    WriteEvery::Mid,
                    WriteEvery::Fast,
                ] {
                    button {
                        key: "{w.label()}",
                        class: if selected == w { "pg-fail-pill is-on" } else { "pg-fail-pill" },
                        onclick: move |_| on_select.call(w),
                        "{w.label()}"
                    }
                }
            }
        }
    }
}

/// The "Disruptive" write switch: Yes / No. Reuses the failure-switch styling.
#[component]
fn DisruptiveSwitch(selected: bool, on_select: EventHandler<bool>) -> Element {
    rsx! {
        div { class: "pg-fail",
            span { class: "pg-fail-label", "Disruptive" }
            div { class: "pg-fail-pills",
                for (val , text) in [(true, "Yes"), (false, "No")] {
                    button {
                        key: "{text}",
                        class: if selected == val { "pg-fail-pill is-on" } else { "pg-fail-pill" },
                        onclick: move |_| on_select.call(val),
                        "{text}"
                    }
                }
            }
        }
    }
}

/// A row of per-node toggle buttons plus an "All" toggle.
#[component]
fn SelectBar(
    label: &'static str,
    n: usize,
    selected: Signal<BTreeSet<usize>>,
    on_toggle: EventHandler<usize>,
    on_all: EventHandler<()>,
) -> Element {
    let all_selected = *selected.read() == (0..n).collect::<BTreeSet<usize>>();
    rsx! {
        div { class: "pg-row",
            span { class: "pg-label", "{label}" }
            div { class: "pg-ids",
                for id in 0..n {
                    button {
                        key: "{id}",
                        class: if selected.read().contains(&id) { "pg-id is-on" } else { "pg-id" },
                        onclick: move |_| on_toggle.call(id),
                        "{id}"
                    }
                }
                button {
                    class: if all_selected { "pg-id pg-id-all is-on" } else { "pg-id pg-id-all" },
                    onclick: move |_| on_all.call(()),
                    "All"
                }
            }
        }
    }
}
