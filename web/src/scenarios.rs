//! Hardcoded walkthrough scenario canvases.
//!
//! Each `SimFigure` in the walkthrough names a scenario (via the `:::figure`
//! marker in `content/*.md`); this module maps that name to a [`ScenarioSpec`] —
//! a short title, the deterministic `lease_sim` [`Scenario`] to run, and the tick
//! at which the run stops on its own. [`ScenarioCanvas`] then renders it exactly
//! like the playground's display (the shared [`crate::sim_view`] widgets): a
//! titled canvas over the same Play/Pause/Resume + timeline + status run bar and
//! grant bars, but replaying a fixed scenario rather than a user-built one.
//!
//! Unlike the playground (fresh random seed per Play, plays until the tick cap),
//! a scenario replays the *same* seeded run every time and stops at its own
//! predetermined tick — a self-contained illustration of one idea.

use dioxus::prelude::*;
use lease_sim::event::{EventKind, LeaseStatus, MsgKind};
use lease_sim::{Event, Scenario, Time, demos};

use crate::sim_view::{GrantBars, RunBar, RunPhase, SimStage, StopWhen, Topology, use_sim_run};

/// Ticks the run keeps playing past its milestone event before halting, so the
/// stopping frame can be read rather than snapping away the instant it lands.
const LEAD_OUT: Time = 150;

/// A hardcoded scenario canvas: a title, the deterministic scenario to replay,
/// and the condition at which its run stops on its own.
pub struct ScenarioSpec {
    /// Short label shown at the top-left of the canvas.
    pub title: &'static str,
    /// The deterministic (fixed-seed) scenario to run.
    pub scenario: fn() -> Scenario,
    /// When the run stops — an event milestone or a tick — always tick-capped.
    pub stop: StopWhen,
    /// Extra vertical breathing room (unit fraction, each end) for the fit-height
    /// canvas — see [`SimStage`]'s `fit_pad`. 0 for most; a busy all-to-all mesh
    /// (roster) sets a little so its off-axis timer stacks aren't clipped.
    pub fit_pad: f64,
}

/// True for the event "the grantor (node 0) received a `RenewReply`" — one steady-
/// state renewal round-tripping back to the grantor.
fn grantor_got_renew_reply(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::MessageDelivered {
            to: 0,
            kind: MsgKind::RenewReply,
            ..
        }
    )
}

/// True for the event "the grantee's (node 1) lease expired". In the guard-reply-
/// lost scenario the grantee never activates, so its only `Expired` transition is
/// its guard window `A'` lapsing — the *later* of the two guard timers to expire
/// (the grantor's own guard give-up fires a bit earlier and emits no event, since
/// its window is anchored at the earlier `Guard` send), i.e. the moment both guard
/// timers have expired.
fn grantee_lease_expired(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::GranteeLease {
            status: LeaseStatus::Expired,
            ..
        }
    )
}

/// True for the event "the grantor (node 0) received a `RevokeReply`" — the
/// grantee's acknowledgement that it dropped the revoked lease, round-tripped
/// back to the grantor.
fn grantor_got_revoke_reply(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::MessageDelivered {
            to: 0,
            kind: MsgKind::RevokeReply,
            ..
        }
    )
}

/// True for the event "either party's lease expired" — a grantor *or* grantee
/// `Expired` transition. Counting to 2 of these lands on the moment *both* sides
/// have expired (they expire independently at different times).
fn either_lease_expired(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::GrantorLease {
            status: LeaseStatus::Expired,
            ..
        } | EventKind::GranteeLease {
            status: LeaseStatus::Expired,
            ..
        }
    )
}

/// True for the event "node 3 sent the grantor a `RenewReply`" — a renewal from
/// the handover's newly-guarded grantee round-tripping back, i.e. the new lease
/// is established and steadily holding.
fn node3_renew_reply(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::MessageDelivered {
            from: 3,
            to: 0,
            kind: MsgKind::RenewReply,
        }
    )
}

/// True for the event "a follower's `Renew` reached the leader (node 0)". In the
/// leader-leases scenario each of the 4 followers grants to node 0, so counting
/// these tallies renew rounds across all grantors — the `4·k`-th is the last
/// follower's k-th, i.e. every follower has renewed the leader at least `k` times.
fn renew_reached_leader(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::MessageDelivered {
            to: 0,
            kind: MsgKind::Renew,
            ..
        }
    )
}

/// True for the event "a `Renew` reached either quorum holder (node 0 or 2)". In
/// the quorum-leases scenario each holder is granted by the other 4 nodes, so
/// counting these across both holders tallies renew rounds cluster-wide — the
/// 24th (2 holders × 4 grantors × 3 rounds) is when the *slower* holder crosses
/// its 3rd round, i.e. both holders have exchanged three renews with everyone.
fn renew_reached_holder(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::MessageDelivered {
            to: 0 | 2,
            kind: MsgKind::Renew,
            ..
        }
    )
}

/// True for the event "a write committed". Anchors the write-disruption canvas's
/// stop: after the commit thaws the cluster, the torn-down leases re-establish and
/// resume renewing; a lead-out past this event covers that recovery window.
fn write_committed(ev: &Event) -> bool {
    matches!(ev.kind, EventKind::WriteCommitted { .. })
}

/// True for the event "any `Renew` was delivered to its grantee". In the roster
/// (all-to-all) scenario every ordered pair holds a lease, so counting these
/// tallies renew rounds mesh-wide — the `L·k`-th (L leases × k) is when the last
/// lease crosses its k-th round, i.e. all leases have renewed at least `k` times.
fn any_renew_delivered(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::MessageDelivered {
            kind: MsgKind::Renew,
            ..
        }
    )
}

/// Resolve a `:::figure <name>` marker to its scenario spec. Unknown names return
/// `None`, so the figure falls back to the plain placeholder.
pub fn lookup(name: &str) -> Option<ScenarioSpec> {
    match name {
        "one-to-one-success" => Some(ScenarioSpec {
            title: "Lease established and kept alive",
            scenario: demos::one_to_one_success,
            // Stop once the grantor has received its 3rd renew reply — the lease
            // is established and visibly holding through a few renew rounds.
            // Capped at 13k ticks as a safety net (the 3rd reply lands well under
            // that on the reliable link).
            stop: StopWhen::OnNthEvent {
                n: 3,
                pred: grantor_got_renew_reply,
                after: 0,
                cap: 13_000,
            },
            fit_pad: 0.0,
        }),
        "one-to-one-guard-reply-lost" => Some(ScenarioSpec {
            title: "Guard msg/reply missing, not established",
            scenario: demos::one_to_one_guard_reply_lost,
            // The grantee acks the guard but that reply is dropped. Stop the moment
            // both guard timers have expired — i.e. when the grantee's guard window
            // lapses (its `Expired`), which trails the grantor's own guard give-up
            // (anchored at the earlier `Guard` send). By then neither side
            // established; a short lead-out lets the expiry read before halting.
            // Capped at 4k as a safety net.
            stop: StopWhen::OnNthEvent {
                n: 1,
                pred: grantee_lease_expired,
                after: LEAD_OUT,
                cap: 4_000,
            },
            fit_pad: 0.0,
        }),
        "one-to-one-revoked" => Some(ScenarioSpec {
            title: "Proactively revoked by grantor",
            scenario: demos::one_to_one_revoked,
            // Establishes, exchanges two renews, then the grantor revokes; the
            // grantee drops its hold and acks. Stop when that revoke ack reaches
            // the grantor. Capped at 5k as a safety net (the ack lands ~2.5k).
            stop: StopWhen::OnNthEvent {
                n: 1,
                pred: grantor_got_revoke_reply,
                after: LEAD_OUT,
                cap: 5_000,
            },
            fit_pad: 0.0,
        }),
        "one-to-one-renew-replies-lost" => Some(ScenarioSpec {
            title: "Failure on the link, lease expires",
            scenario: demos::one_to_one_renew_replies_lost,
            // Establishes and renews once, then all renew acks are dropped. The
            // grantor gives up renewing and both sides lapse. Stop when both have
            // expired (the 2nd of the two `Expired` events). Capped at 9k as a
            // safety net (both-expired lands ~5.9k).
            stop: StopWhen::OnNthEvent {
                n: 2,
                pred: either_lease_expired,
                after: LEAD_OUT,
                cap: 9_000,
            },
            fit_pad: 0.0,
        }),
        "lease-manager-handover" => Some(ScenarioSpec {
            title: "Manager hands grants to 1,2 then 2,3",
            scenario: demos::lease_manager_handover,
            // Grants to 1 and 2, then hands 1's slot to 3 (revoke 1, keep 2, guard
            // 3). Stop once node 3 — the newly-guarded grantee — has renewed twice,
            // so the handover is visibly complete and steady. Capped at 8k (node
            // 3's 2nd renew reply lands ~3.3k).
            stop: StopWhen::OnNthEvent {
                n: 2,
                pred: node3_renew_reply,
                after: LEAD_OUT,
                cap: 8_000,
            },
            fit_pad: 0.0,
        }),
        "leader-leases" => Some(ScenarioSpec {
            title: "Every node leases their believed leader",
            scenario: demos::leader_leases,
            // 5 nodes, all-to-one: followers 1..4 each grant to leader 0. Stop once
            // the leader has been renewed by all followers through 3 rounds each —
            // the 12th `Renew` to reach it (4 grantors × 3), by which point every
            // follower has renewed three times. Capped at 8k (the 12th lands ~2.3k).
            stop: StopWhen::OnNthEvent {
                n: 12,
                pred: renew_reached_leader,
                after: LEAD_OUT,
                cap: 8_000,
            },
            fit_pad: 0.0,
        }),
        "quorum-leases" => Some(ScenarioSpec {
            title: "A chosen subset of nodes hold read leases",
            scenario: demos::quorum_leases,
            // 5 nodes, all-to-many: holders 0 and 2 are each granted by the other
            // four; no writes. Stop once both holders have exchanged three renews
            // with everyone — the 24th `Renew` to reach a holder (2 holders × 4
            // grantors × 3), when the slower holder crosses its 3rd round. Capped
            // at 8k (the 24th lands ~2.4k).
            stop: StopWhen::OnNthEvent {
                n: 24,
                pred: renew_reached_holder,
                after: LEAD_OUT,
                cap: 8_000,
            },
            fit_pad: 0.0,
        }),
        "quorum-leases-write-disruption" => Some(ScenarioSpec {
            title: "Write folds lease logic and disrupts read leases",
            scenario: demos::quorum_leases_write_disruption,
            // Same two-holder setup, then a disruptive write at t=2000 tears every
            // lease down; on commit the cluster re-establishes (re-guards) and
            // resumes renewing. Stop a lead-out past the commit — long enough for
            // both holders to re-establish and exchange ~3 renew rounds again
            // (commit ~2.5k, three fresh rounds done ~4.7k). Capped at 9k.
            stop: StopWhen::OnNthEvent {
                n: 1,
                pred: write_committed,
                after: 2_300,
                cap: 9_000,
            },
            fit_pad: 0.0,
        }),
        "roster-leases" => Some(ScenarioSpec {
            title: "All-to-all leasing on a roster metadata",
            scenario: demos::roster_leases,
            // 5 nodes, all-to-all: 20 leases (every ordered pair). Stop once every
            // lease has renewed at least three rounds — the 60th `Renew` delivered
            // (20 leases × 3), when the last lease crosses its 3rd round. Capped at
            // 8k (the 60th lands ~2.3k).
            stop: StopWhen::OnNthEvent {
                n: 60,
                pred: any_renew_delivered,
                after: LEAD_OUT,
                cap: 8_000,
            },
            // The all-to-all mesh's off-axis nodes carry tall pushed-out timer
            // stacks; a little extra room keeps them from clipping the box.
            fit_pad: 0.05,
        }),
        _ => None,
    }
}

/// A self-contained scenario canvas: a titled animated stage over the shared run
/// bar and grant bars, replaying the fixed scenario named by `name`. Falls back
/// to a plain placeholder for an unknown name.
#[component]
pub fn ScenarioCanvas(name: String) -> Element {
    let Some(spec) = lookup(&name) else {
        return rsx! {
            figure { class: "sim-figure",
                div { class: "sim-placeholder",
                    span { "simulation canvas" }
                }
            }
        };
    };

    // Derive the static topology once from the scenario (single source of truth),
    // and keep the scenario/stop-condition to (re)start the run on Play.
    let scenario = (spec.scenario)();
    let topology = Topology::from_scenario(&scenario);
    let stop = spec.stop;
    let fit_pad = spec.fit_pad;
    let scenario_fn = spec.scenario;

    // Same fixed playback pace as the playground (no fast-forward toggle shown).
    let mut run = use_sim_run();

    let ph = run.phase();
    let rec_len = run.rec_len();
    let current_frame = run.frame();
    let cursor_frac = run.cursor_frac();
    let bars = run.grant_bars(&topology);

    let status = match ph {
        RunPhase::Generating => rsx! {
            span { class: "pg-spinner" }
            span { class: "pg-status", "sim running" }
        },
        RunPhase::Paused => rsx! {
            span { class: "pg-status", "run stopped" }
        },
        RunPhase::Stopped => rsx! {
            span { class: "pg-status", "run complete" }
        },
        RunPhase::Idle => rsx! {
            span { class: "pg-status pg-status-hint", "press Play" }
        },
    };

    rsx! {
        figure { class: "sim-figure",
            // The whole scenario — title, canvas, run bar, grant bars — sits in one
            // light-gray box, marking it as a single self-contained animation unit.
            div { class: "pg-root sc-root",
                // Scenario title, above the canvas (not inside it).
                div { class: "sc-title", "{spec.title}" }
                // The canvas: static topology while idle, animated frame otherwise.
                // Height is cropped to the content (`fit_height`).
                SimStage {
                    topology: topology.clone(),
                    frame: current_frame,
                    fit_height: true,
                    fit_pad,
                }

                RunBar {
                    phase: ph,
                    runnable: true,
                    rec_len,
                    cur: run.cursor(),
                    now_ticks: run.now_ticks(),
                    end_ticks: run.end_ticks(),
                    fast_forward: run.fast_forward(),
                    show_ff: false,
                    lock_when_stopped: true,
                    on_play: move |_| run.start((scenario_fn)(), stop),
                    on_pause: move |_| run.pause(),
                    on_resume: move |_| run.resume(),
                    on_restart: move |_| run.reset(),
                    on_scrub: move |v| run.set_cursor(v),
                    on_toggle_ff: move |_| run.toggle_fast_forward(),
                    status,
                }

                GrantBars {
                    topology: topology.clone(),
                    bars,
                    cursor_frac,
                    rec_len,
                    grantees_only: true,
                    draggable: matches!(ph, RunPhase::Paused | RunPhase::Stopped),
                }
            }
        }
    }
}
