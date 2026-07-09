//! The simulator playground: scenario-setup controls over a live canvas that
//! reflects the chosen nodes and grantor -> grantee lease relationships.
//!
//! A `Play` press builds a [`Scenario`] from the current knobs and *generates*
//! the run live: the `lease_sim` engine is advanced [`FRAME_TICKS`] ticks per
//! wall-clock step, each step recording a `Frame` and animating on the canvas,
//! until every selected grantee has held a majority of leases for at least
//! `SETTLE_MULT·T_expire` (failure-free for now). While it settles the timeline
//! slider is inert and a spinner shows; once settled, the whole recorded run
//! stays put and the slider freely scrubs it. Any change to a scenario knob
//! discards the run and returns the canvas to static editing.

use std::collections::BTreeSet;
use std::time::Duration;

use dioxus::prelude::*;
use lease_sim::frame::ring_layout;
use lease_sim::{Engine, Frame, LeaseStatus, MsgKind, NodeId, Point, Scenario, Time};

/// Node-count bounds for the slider.
const MIN_NODES: usize = 2;
const MAX_NODES: usize = 9;

/// Global ticks between recorded frames — the time resolution of the run and
/// the granularity the timeline slider scrubs at. Kept fine for smooth motion.
const FRAME_TICKS: Time = 5;
/// Wall-clock interval between generation repaints.
const RENDER_MS: u32 = 18;
/// Frames advanced per repaint while generating. Resolution stays fine
/// (`FRAME_TICKS`) while the run still settles in a few seconds of wall clock:
/// `FRAMES_PER_STEP · FRAME_TICKS / RENDER_MS` ticks of sim per real ms.
const FRAMES_PER_STEP: usize = 3;
/// Hidden safety cap on how long a run may generate if it never settles (e.g.
/// fewer grantors than the majority threshold). Hitting it ends the run as
/// `Capped`.
const MAX_TICKS: Time = 60_000;
/// A run settles once every grantee has held a majority continuously for this
/// many `T_expire` lifetimes.
const SETTLE_MULT: Time = 2;

/// A starting-point scenario shape, mirroring the four algorithm levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Preset {
    OneToOne,
    Leader,
    Quorum,
    Roster,
}

impl Preset {
    /// (node count, grantor ids, grantee ids) this preset expands to.
    fn expand(self) -> (usize, Vec<usize>, Vec<usize>) {
        match self {
            Preset::OneToOne => (2, vec![0], vec![1]),
            Preset::Leader => (5, (0..5).collect(), vec![0]),
            Preset::Quorum => (5, (0..5).collect(), vec![1, 3]),
            Preset::Roster => (5, (0..5).collect(), (0..5).collect()),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Preset::OneToOne => "One-to-One",
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
    /// A run is being generated live (settling animation playing out).
    Generating,
    /// Finished: the settle condition held. The slider now scrubs the run.
    Settled,
    /// Manually stopped mid-generation via the Stop button. Like [`Phase::Settled`]
    /// for scrubbing; just labeled as a user stop rather than an auto-settle.
    Stopped,
    /// Finished at the [`MAX_TICKS`] cap without ever settling — e.g. fewer
    /// grantors than the majority threshold, so grantees can never reach it.
    Capped,
}

/// Bookkeeping carried across generation steps.
#[derive(Clone, PartialEq)]
struct GenState {
    /// Global time of the next frame to generate.
    t: Time,
    /// Global time each node most recently reached majority (reset on a drop).
    major_since: Vec<Option<Time>>,
    /// Majority threshold for this run.
    maj: usize,
    /// Continuous-hold window required to settle, in ticks.
    settle: Time,
    /// The grantees whose settling terminates the run.
    grantees: BTreeSet<usize>,
}

/// Strict majority (quorum-intersection threshold) for a cluster of size `n`.
fn majority(n: usize) -> usize {
    n / 2 + 1
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
/// append-only as the simulation generates, so completed runs never change.
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
/// source of truth that persists after a run settles), so bars stay put once
/// generation stops.
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

/// CSS color for a grant-status run, as a plain comma-free hex string. When no
/// grant is held it matches the empty track (`--surface` = #f4f5f7); otherwise
/// it's a green (`--grantee` = #2f8f5b) mixed toward white, deepening with how
/// many of the node's possible grants (`max`) are held.
///
/// Returns hex (not `color-mix(...)`/`var(...)`) on purpose: Dioxus's inline
/// `style` string parser drops values containing commas, so a `color-mix` or
/// gradient silently renders as no style at all.
fn grant_color(grants: usize, max: usize) -> String {
    if grants == 0 || max == 0 {
        return "#f4f5f7".to_string(); // matches --surface (empty track)
    }
    // Grantee green (#2f8f5b) mixed toward white; deeper with more grants.
    const G: (f64, f64, f64) = (0x2f as f64, 0x8f as f64, 0x5b as f64);
    let frac = (grants as f64 / max as f64).clamp(0.0, 1.0);
    let green = 0.30 + 0.55 * frac; // 30%..85% green, remainder white
    let mix = |c: f64| (c * green + 255.0 * (1.0 - green)).round() as u8;
    format!("#{:02x}{:02x}{:02x}", mix(G.0), mix(G.1), mix(G.2))
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

/// Build a failure-free scenario from the chosen knobs: every node initiates
/// the leases it grants (per-poll chance, so guarding starts at a staggered
/// random time), links are reliable, and no node fails.
fn build_scenario(n: usize, grantors: &BTreeSet<usize>, grantees: &BTreeSet<usize>) -> Scenario {
    let mut s = Scenario::new(n)
        .seed(1)
        .duration(MAX_TICKS)
        .all_nodes(|nc| nc.initiate_chance = 0.5);
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

/// CSS class for a message glyph, grouped (colored) by protocol phase. Reply
/// kinds add `is-reply`, which lightens the phase color a touch.
fn msg_class(kind: MsgKind) -> &'static str {
    match kind {
        MsgKind::Guard => "pg-msg is-guard",
        MsgKind::GuardReply => "pg-msg is-guard is-reply",
        MsgKind::Renew => "pg-msg is-renew",
        MsgKind::RenewReply => "pg-msg is-renew is-reply",
        MsgKind::Revoke => "pg-msg is-revoke",
    }
}

/// Whether a message is a grantee's acknowledgement reply — drawn with a small
/// "thumbs-up" badge over its base glyph.
fn msg_is_reply(kind: MsgKind) -> bool {
    matches!(kind, MsgKind::GuardReply | MsgKind::RenewReply)
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

/// Symbol for an in-flight message, drawn inside its positioned `.pg-msg` chip:
/// a shield for guard-phase messages, a circular "renew" arrow for renewals, a
/// dot for revokes. Reply kinds (`*Reply`) overlay a small thumbs-up badge to
/// mark them as acknowledgements. Colored by phase via `currentColor` (set on
/// `.pg-msg` in CSS).
#[component]
fn MsgGlyph(kind: MsgKind) -> Element {
    // Guard-phase → shield; renewals → circular refresh arrow (Material-style);
    // revoke → a plain filled dot.
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

#[component]
pub fn Playground() -> Element {
    let mut node_count = use_signal(|| 2usize);
    let mut grantors = use_signal(|| BTreeSet::from([0usize]));
    let mut grantees = use_signal(|| BTreeSet::from([1usize]));
    let mut preset = use_signal(|| Some(Preset::OneToOne));

    // Run state: lifecycle phase, the live engine (only during generation), the
    // recorded frames, per-run bookkeeping, and the scrub cursor.
    let mut phase = use_signal(|| Phase::Idle);
    let mut engine = use_signal(|| None::<Engine>);
    let mut frames = use_signal(Vec::<Frame>::new);
    let mut gen_state = use_signal(|| None::<GenState>);
    let mut cursor = use_signal(|| 0usize);

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
    // live (rather than all at once) is what makes the settling visible; the
    // batch keeps wall-clock pace reasonable at a fine time resolution. When the
    // settle condition holds (or the cap is hit) it stops, leaving the recorded
    // frames for scrubbing.
    use_future(move || async move {
        loop {
            gloo_timers::future::sleep(Duration::from_millis(RENDER_MS as u64)).await;
            if *phase.peek() != Phase::Generating {
                continue;
            }
            let mut last_idx = None;
            let mut done: Option<bool> = None;
            for _ in 0..FRAMES_PER_STEP {
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
                // Update majority-hold tracking and decide whether the run is done.
                let (finished, settled_ok) = {
                    let mut g = gen_state.write();
                    let gs = g.as_mut().unwrap();
                    for (node, since) in gs.major_since.iter_mut().enumerate() {
                        let held = frame
                            .leases
                            .iter()
                            .filter(|b| {
                                b.grantee == node && b.grantee_status == LeaseStatus::Active
                            })
                            .count()
                            + 1; // +1 for the node's implicit self-grant
                        if held >= gs.maj {
                            since.get_or_insert(t);
                        } else {
                            *since = None;
                        }
                    }
                    let settled = !gs.grantees.is_empty()
                        && gs
                            .grantees
                            .iter()
                            .all(|&h| gs.major_since[h].is_some_and(|t0| t - t0 >= gs.settle));
                    if settled {
                        (true, true)
                    } else if t >= MAX_TICKS {
                        (true, false)
                    } else {
                        gs.t = t + FRAME_TICKS;
                        (false, false)
                    }
                };
                // Append the frame; track the newest index for the live view.
                let mut fr = frames.write();
                fr.push(frame);
                last_idx = Some(fr.len() - 1);
                if finished {
                    done = Some(settled_ok);
                    break;
                }
            }
            // Keep the live view on the newest frame of this batch.
            if let Some(idx) = last_idx {
                cursor.set(idx);
            }
            if let Some(settled_ok) = done {
                phase.set(if settled_ok {
                    Phase::Settled
                } else {
                    Phase::Capped
                });
                engine.set(None); // recorded frames remain; free the engine
            }
        }
    });

    // Apply a preset: set the count and both selection sets in one shot.
    let mut apply_preset = move |p: Preset| {
        let (n, gs, hs) = p.expand();
        node_count.set(n);
        grantors.set(gs.into_iter().collect());
        grantees.set(hs.into_iter().collect());
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
        let scenario = build_scenario(n, &gs, &hs);
        let settle = SETTLE_MULT * scenario.params.t_lease;
        engine.set(Some(Engine::new(scenario)));
        frames.write().clear();
        cursor.set(0);
        gen_state.set(Some(GenState {
            t: 0,
            major_since: vec![None; n],
            maj: majority(n),
            settle,
            grantees: hs,
        }));
        phase.set(Phase::Generating);
    };

    // Stop: manually end an in-progress generation at the current frame, keeping
    // the recorded run for scrubbing (a user-declared settle).
    let on_stop = move |_| {
        if *phase.peek() == Phase::Generating {
            phase.set(Phase::Stopped);
            engine.set(None); // recorded frames remain; free the engine
        }
    };

    // The run button toggles Play/Stop depending on whether a run is generating.
    let generating = ph == Phase::Generating;
    let scrubbable = matches!(ph, Phase::Settled | Phase::Stopped | Phase::Capped);

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
    // truth, which persists after a run settles). A node that is both a grantor
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
                    for p in [Preset::OneToOne, Preset::Leader, Preset::Quorum, Preset::Roster] {
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
                            div {
                                key: "m{i}",
                                class: msg_class(m.kind),
                                style: format!(
                                    "left: {:.3}%; top: {:.3}%; opacity: {:.3};",
                                    m.pos.x * 100.0,
                                    m.pos.y * 100.0,
                                    msg_opacity(m.progress),
                                ),
                                MsgGlyph { kind: m.kind }
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
                            span { class: "pg-node-id", "{id}" }
                        }
                    }
                }
            }

            // Run bar: a 3-column grid — [control | track | status] — shared by
            // the axis and grant-bar rows below so slider, axis, and every bar
            // line up on the same track. Control cell = Play button + clock glyph.
            div { class: "pg-runbar",
                div { class: "pg-runctrl",
                    // Toggles Play → Stop while a run generates; Stop manually
                    // ends it as a user-declared settle.
                    if generating {
                        button {
                            class: "pg-btn is-stop",
                            onclick: on_stop,
                            "Stop"
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
                                span { class: "pg-status", "settling…" }
                            },
                            Phase::Settled => rsx! {
                                span { class: "pg-status", "✓ run settled" }
                            },
                            Phase::Stopped => rsx! {
                                span { class: "pg-status", "✓ run stopped" }
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

            // Timing axis on the shared grid: an empty control cell, then the
            // track (scrub time centered, run end at the right), then an empty
            // status cell — so it lines up exactly under the slider.
            div { class: "pg-timeaxis",
                span {}
                span { class: "pg-axis-track",
                    span { class: "pg-axis-spacer" }
                    span { class: "pg-time-readout",
                        "t = "
                        strong { "{now_ticks}" }
                    }
                    span { class: "pg-axis-end", "max {end_ticks} ticks" }
                }
                span {}
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
                                        "data-hint": "{run.grants} grant{plural(run.grants)} held · {run.frames as i64 * FRAME_TICKS} ticks",
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
                TConst { name: "expire", hint: "Lease expiration timeout" }
                " ≈ "
                TConst { name: "guard", hint: "Guard phase timeout" }
                " ≈ 2.5 x "
                TConst { name: "renew", hint: "Lease renewal interval" }
                span { class: "pg-sep", ";" }
                TConst { name: "renew", hint: "Lease renewal interval" }
                " ≈ 3 x "
                TConst { name: "msg", hint: "Average message delivery delay" }
                span { class: "pg-sep", ";" }
                TConst { name: "msg", hint: "Average message delivery delay" }
                " ≈ 2 x "
                TConst { name: "Δ", hint: "Bounded clock drift" }
                " with ±40% jitter"
                span { class: "pg-sep", ";" }
                TConst { name: "Δ", hint: "Bounded clock drift" }
                " is 100 ticks."
            }
        }
    }
}

/// A boxed timing constant `T_name`, math-styled with a subscript and a hover
/// tooltip explaining what it is. `name` is the subscript text (e.g. "expire").
#[component]
fn TConst(name: &'static str, hint: &'static str) -> Element {
    rsx! {
        span { class: "pg-tconst", "data-hint": "{hint}",
            "T"
            sub { "{name}" }
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
