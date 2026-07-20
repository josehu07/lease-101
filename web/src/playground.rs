//! The simulator playground: scenario-setup controls over a live canvas that
//! reflects the chosen nodes and grantor -> grantee lease relationships.
//!
//! A `Play` press builds a [`Scenario`] from the current knobs and *generates*
//! the run live: the `lease_sim` engine is advanced [`crate::sim_view::FRAME_TICKS`] ticks per
//! wall-clock step, each step recording a `Frame` and animating on the canvas.
//! The run keeps playing until the user pauses it or it reaches the [`MAX_TICKS`]
//! cap. While running the timeline slider is inert and a spinner shows; once
//! paused (or capped) the whole recorded run stays put and the slider freely
//! scrubs it, and a paused run can be resumed from where it left off. Any change
//! to a scenario knob discards the run and returns the canvas to static editing.
//!
//! The canvas, run bar, grant bars, and the live-generation loop are the shared
//! [`crate::sim_view`] widgets (identical to the walkthrough's scenario canvases);
//! this module owns only the scenario-editing controls that build the run.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use lease_sim::{MsgKind, Scenario, Time};

use crate::sim_view::{
    GrantBars, RunBar, RunPhase, SimStage, StopWhen, TConst, Topology, majority, use_sim_run,
};

/// Node-count bounds for the slider.
const MIN_NODES: usize = 2;
const MAX_NODES: usize = 9;

/// Cap on how long a run may play. A run keeps generating until the user pauses
/// it or it reaches this many ticks; hitting the cap ends the run (`Stopped`).
const MAX_TICKS: Time = 60_000;

/// A per-message-kind drop rate: one of five levels (Off / 1% / 10% / 30% /
/// 100%). The selectable values for each msg-drop switch (Guard, its reply,
/// Renew, its reply).
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
    /// `(guard_fail, renew_fail, write_every, write_disruptive)`.
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

/// A fresh random seed for a run, drawn from the browser's `Math.random()`. The
/// engine is deterministic *given* a seed; picking a new one per Play is what
/// makes each run's stochastic drops/timings differ.
fn fresh_seed() -> u64 {
    let hi = (js_sys::Math::random() * (1u64 << 32) as f64) as u64;
    let lo = (js_sys::Math::random() * (1u64 << 32) as f64) as u64;
    (hi << 32) | lo
}

/// Build a scenario from the chosen knobs: every node initiates the leases it
/// grants (per-poll chance, so guarding starts at a staggered random time),
/// links have no baseline loss, and no node fails. The four msg-drop switches
/// (Guard, GuardReply, Renew, RenewReply) inject a per-kind drop probability on
/// top, and the write switches set the leader's write cadence and disruptiveness.
#[allow(clippy::too_many_arguments)]
fn build_scenario(
    topology: &Topology,
    guard_fail: FailRate,
    guard_reply_fail: FailRate,
    renew_fail: FailRate,
    renew_reply_fail: FailRate,
    write_every: WriteEvery,
    write_disruptive: bool,
    seed: u64,
) -> Scenario {
    let n = topology.n;
    let mut s = Scenario::new(n)
        .seed(seed)
        .duration(MAX_TICKS)
        .all_nodes(|nc| nc.initiate_chance = 0.5)
        .kind_drop(MsgKind::Guard, guard_fail.prob())
        .kind_drop(MsgKind::GuardReply, guard_reply_fail.prob())
        .kind_drop(MsgKind::Renew, renew_fail.prob())
        .kind_drop(MsgKind::RenewReply, renew_reply_fail.prob())
        .writes(write_every.interval(), write_disruptive);
    for &g in &topology.grantors {
        for &h in &topology.grantees {
            if g != h && g < n && h < n {
                s = s.lease(g, h);
            }
        }
    }
    s
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

#[component]
pub fn Playground() -> Element {
    let mut node_count = use_signal(|| 2usize);
    let mut grantors = use_signal(|| BTreeSet::from([0usize]));
    let mut grantees = use_signal(|| BTreeSet::from([1usize]));
    let mut preset = use_signal(|| Some(Preset::OneToOne));
    // Per-kind message-failure switches; not part of the scenario *shape*, so
    // they don't clear the chosen preset when changed. Each request kind (Guard,
    // Renew) also has a reply-drop switch for its ack (GuardReply, RenewReply).
    let mut guard_fail = use_signal(|| FailRate::Tiny);
    let mut guard_reply_fail = use_signal(|| FailRate::Tiny);
    let mut renew_fail = use_signal(|| FailRate::Tiny);
    let mut renew_reply_fail = use_signal(|| FailRate::Tiny);
    // Write-load switches: how often the leader serves a write, and whether that
    // write is disruptive. Like the failure switches, not part of the shape.
    let mut write_every = use_signal(|| WriteEvery::Never);
    let mut write_disruptive = use_signal(|| false);

    // The live run: recorded frames, lifecycle, and the shared generation loop.
    let mut run = use_sim_run();

    // Discard any run, returning the canvas to static editing.
    let mut reset_sim = move || run.reset();

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
        // Reply drops aren't part of a preset; reset them to the 1% default.
        guard_reply_fail.set(FailRate::Tiny);
        renew_reply_fail.set(FailRate::Tiny);
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
    let topology = Topology::new(
        n,
        grantors.read().iter().copied(),
        grantees.read().iter().copied(),
    );
    let runnable = topology.has_leases();

    // Snapshot run state for this render.
    let ph = run.phase();
    let rec_len = run.rec_len();
    let current_frame = run.frame();
    let cursor_frac = run.cursor_frac();
    let bars = run.grant_bars(&topology);

    // Play: (re)build the scenario and begin a fresh live generation.
    let on_play = move |_| {
        if !runnable {
            return;
        }
        let topo = Topology::new(
            node_count(),
            grantors.read().iter().copied(),
            grantees.read().iter().copied(),
        );
        let scenario = build_scenario(
            &topo,
            guard_fail(),
            guard_reply_fail(),
            renew_fail(),
            renew_reply_fail(),
            write_every(),
            write_disruptive(),
            fresh_seed(),
        );
        run.start(scenario, StopWhen::AtTick(MAX_TICKS));
    };

    let status = match ph {
        RunPhase::Generating => rsx! {
            span { class: "pg-spinner" }
            span { class: "pg-status", "sim running" }
        },
        RunPhase::Paused => rsx! {
            span { class: "pg-status", "run stopped" }
        },
        RunPhase::Stopped => rsx! {
            span { class: "pg-status is-error", "✗ ticks limit" }
        },
        RunPhase::Idle => rsx! {
            span { class: "pg-status pg-status-hint",
                if runnable {
                    "press Play"
                } else {
                    "select a grantor & grantee"
                }
            }
        },
    };

    rsx! {
        div { class: "pg-root",
            // Row 1: presets.
            div { class: "pg-row",
                span {
                    class: "pg-label pg-hint",
                    "data-hint": "Preset scenario",
                    "Preset"
                }
                div { class: "pg-pills",
                    for p in [
                        Preset::OneToOne,
                        Preset::LeaseManager,
                        Preset::Leader,
                        Preset::Quorum,
                        Preset::Roster,
                    ]
                    {
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
                span {
                    class: "pg-label pg-hint",
                    "data-hint": "Number of nodes; majority is ⌊n/2⌋+1 (incl. self)",
                    "Nodes"
                }
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
                    "  (incl. self)"
                }
            }

            // Row 3: grantor selection.
            SelectBar {
                label: "Grantors",
                hint: "Nodes that grant leases out",
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
                hint: "Nodes that receive lease grants",
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

            // Row 5: per-kind message-failure switches. Each request kind stacks
            // its reply-drop switch directly beneath it (Guard / its Reply, Renew /
            // its Reply).
            div { class: "pg-row",
                span {
                    class: "pg-label pg-hint",
                    "data-hint": "Chance of randomly dropping a msg of type",
                    "Msg drop"
                }
                div { class: "pg-fails",
                    div { class: "pg-fail-stack",
                        FailSwitch {
                            label: "Guard",
                            selected: guard_fail(),
                            on_select: move |r| {
                                guard_fail.set(r);
                                reset_sim();
                            },
                        }
                        FailSwitch {
                            label: "Reply",
                            selected: guard_reply_fail(),
                            on_select: move |r| {
                                guard_reply_fail.set(r);
                                reset_sim();
                            },
                        }
                    }
                    div { class: "pg-fail-stack",
                        FailSwitch {
                            label: "Renew",
                            selected: renew_fail(),
                            on_select: move |r| {
                                renew_fail.set(r);
                                reset_sim();
                            },
                        }
                        FailSwitch {
                            label: "Reply",
                            selected: renew_reply_fail(),
                            on_select: move |r| {
                                renew_reply_fail.set(r);
                                reset_sim();
                            },
                        }
                    }
                }
            }

            // Row 6: write-load switches.
            div { class: "pg-row",
                span {
                    class: "pg-label pg-hint",
                    "data-hint": "How often the leader serves a write, and will writes disrupt leases",
                    "Writes"
                }
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
            // (Square box, no title — the playground keeps the full stage.)
            SimStage { topology: topology.clone(), frame: current_frame }

            RunBar {
                phase: ph,
                runnable,
                rec_len,
                cur: run.cursor(),
                now_ticks: run.now_ticks(),
                end_ticks: run.end_ticks(),
                fast_forward: run.fast_forward(),
                on_play,
                on_pause: move |_| run.pause(),
                on_resume: move |_| run.resume(),
                on_restart: move |_| reset_sim(),
                on_scrub: move |v| run.set_cursor(v),
                on_toggle_ff: move |_| run.toggle_fast_forward(),
                status,
            }

            GrantBars {
                topology: topology.clone(),
                bars,
                cursor_frac,
                rec_len,
                draggable: matches!(ph, RunPhase::Paused | RunPhase::Stopped),
            }

            p { class: "pg-caption",
                "Hardcoded: "
                TConst {
                    name: "expire",
                    hint: "Lease expiration timeout",
                    ticks: "1500 ticks",
                }
                " ≈ "
                TConst {
                    name: "guard",
                    hint: "Guard phase timeout",
                    ticks: "1500 ticks",
                }
                " ≈ 2.5 x "
                TConst {
                    name: "renew",
                    hint: "Lease renewal interval",
                    ticks: "600 ticks",
                }
                span { class: "pg-sep", ";" }
                TConst {
                    name: "renew",
                    hint: "Lease renewal interval",
                    ticks: "600 ticks",
                }
                " ≈ 3 x "
                TConst {
                    name: "msg",
                    hint: "Average message delivery delay",
                    ticks: "~200 ticks avg",
                }
                span { class: "pg-sep", ";" }
                TConst {
                    name: "msg",
                    hint: "Average message delivery delay",
                    ticks: "~200 ticks avg",
                }
                " ≈ 2 x "
                TConst {
                    name: "Δ",
                    hint: "Bounded clock drift",
                    ticks: "100 ticks",
                }
                " with ±40% jitter"
                span { class: "pg-sep", ";" }
                TConst {
                    name: "Δ",
                    hint: "Bounded clock drift",
                    ticks: "100 ticks",
                }
                " is 100 ticks."
            }
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
                for r in [FailRate::Off, FailRate::Tiny, FailRate::Low, FailRate::Some, FailRate::All] {
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

/// The "Every" write switch: how often the leader serves a write.
#[component]
fn WriteEverySwitch(selected: WriteEvery, on_select: EventHandler<WriteEvery>) -> Element {
    rsx! {
        div { class: "pg-fail",
            span { class: "pg-fail-label", "Every" }
            div { class: "pg-fail-pills",
                for w in [WriteEvery::Never, WriteEvery::Slow, WriteEvery::Mid, WriteEvery::Fast] {
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

/// The "Disruptive" write switch: Yes / No.
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
    hint: &'static str,
    n: usize,
    selected: Signal<BTreeSet<usize>>,
    on_toggle: EventHandler<usize>,
    on_all: EventHandler<()>,
) -> Element {
    let all_selected = *selected.read() == (0..n).collect::<BTreeSet<usize>>();
    rsx! {
        div { class: "pg-row",
            span { class: "pg-label pg-hint", "data-hint": "{hint}", "{label}" }
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
