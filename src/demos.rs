//! Ready-made demo scenarios for the walkthrough figures.
//!
//! Each function returns a fully-configured [`Scenario`] with a *fixed seed*, so
//! the run is fully reproducible: the walkthrough canvases replay an identical
//! run every time (unlike the playground, which draws a fresh seed per Play).
//! Kept in the core crate — rather than the web layer — so their outcomes are
//! natively unit-testable (the WASM view cannot be).
//!
//! These are curated, deterministic *illustrations* of the one-to-one primitive:
//! guarding is driven by a scripted [`Command::Initiate`] at a known tick (not the
//! stochastic per-poll `initiate_chance`), so establishment happens at a stable
//! time regardless of seed.

use crate::clock::Time;
use crate::event::{Command, LeaseId, MsgKind};
use crate::scenario::Scenario;

/// Script a `Command::Initiate` at tick `at` for every lease the scenario
/// declares, so the whole pattern establishes deterministically at a stable time
/// rather than via staggered stochastic initiation.
fn guard_all(mut s: Scenario, at: Time) -> Scenario {
    for l in s.leases.clone() {
        s = s.command(at, Command::Initiate(l));
    }
    s
}

/// The canonical happy-path one-to-one lease: node 0 grants to node 1 over a
/// reliable link, guarding once and then renewing steadily forever. No drops, no
/// writes, no failures — the base primitive working exactly as intended.
pub fn one_to_one_success() -> Scenario {
    Scenario::new(2)
        .seed(0x0170_0170) // fixed: deterministic, reproducible run
        .duration(20_000)
        .lease(0, 1)
        // Guard at a known tick instead of relying on random initiation, so the
        // lease always establishes at the same time.
        .command(
            300,
            Command::Initiate(LeaseId {
                grantor: 0,
                grantee: 1,
            }),
        )
}

/// The guard phase failing: node 0 opens a guard, node 1 receives it and acks,
/// but that `GuardReply` is lost. The grantor hears nothing, so once its guard
/// window (`t_guard - t_delta`) lapses it abandons the attempt and falls back to
/// idle — the lease never establishes. With no re-initiation (`initiate_chance`
/// stays 0) there is exactly one attempt, so it's a single clean illustration of
/// a dropped guard ack stranding establishment.
pub fn one_to_one_guard_reply_lost() -> Scenario {
    Scenario::new(2)
        .seed(0x6a17_d500) // fixed: deterministic, reproducible run
        .duration(20_000)
        .lease(0, 1)
        // Every guard ack is lost, so the one guard attempt below never completes.
        .kind_drop(MsgKind::GuardReply, 1.0)
        .command(
            300,
            Command::Initiate(LeaseId {
                grantor: 0,
                grantee: 1,
            }),
        )
}

/// A lease established, kept alive through a couple of renews, then *proactively
/// revoked*: node 0 guards (t=300), activates, exchanges two renew rounds, then a
/// scripted `Revoke` tells node 1 to drop the lease. The grantee expires its hold
/// and acks with a `RevokeReply`. Illustrates the grantor-initiated end of a lease
/// (vs. passive expiry).
pub fn one_to_one_revoked() -> Scenario {
    let id = LeaseId {
        grantor: 0,
        grantee: 1,
    };
    Scenario::new(2)
        .seed(0x0270_0270) // fixed: deterministic, reproducible run
        .duration(20_000)
        .lease(0, 1)
        .command(300, Command::Initiate(id))
        // Revoke at t=1920: after the 2nd renew's reply has round-tripped back
        // (~t=1895) but before the 3rd renew falls due (~t=1950). Revoking clears
        // `intended`, so that 3rd renew is never sent — the grantor goes straight
        // from "two renews exchanged" to revoking, with no stray renew alongside.
        .command(1920, Command::Revoke(id))
}

/// A lease established, then *passively expiring* because the grantee's renew acks
/// stop arriving. Node 0 guards (t=300), activates, and exchanges one renew round;
/// from t=1300 every `RenewReply` is dropped. The grantor keeps renewing but hears
/// nothing back, so after its renew-reply timeout it gives up renewing; the grantee
/// then stops receiving renews and its hold lapses. Both sides expire — the failure
/// mode where silence, not a revoke, ends the lease.
pub fn one_to_one_renew_replies_lost() -> Scenario {
    Scenario::new(2)
        .seed(0x0370_0370) // fixed: deterministic, reproducible run
        .duration(20_000)
        .lease(0, 1)
        // Let the first renew ack (~t=1000) through, then drop every `RenewReply`
        // from t=1300 on — so the lease establishes and renews once before the
        // acks go silent.
        .kind_drop_from(MsgKind::RenewReply, 1.0, 1300)
        .command(
            300,
            Command::Initiate(LeaseId {
                grantor: 0,
                grantee: 1,
            }),
        )
}

/// The lease-manager pattern with a *handover*: node 0 is a manager granting to
/// potential grantees 1, 2, 3, but never to all at once. It first grants to 1 and
/// 2; after a couple of renew rounds with node 1 it hands that slot to node 3 —
/// revoking 1 (2 is untouched and keeps renewing) and, once 1's revoke is
/// acknowledged, guarding 3. Illustrates independent per-grantee lease lifecycles
/// under one grantor: one lease revoked and re-placed while another rides on.
///
/// The tick script below is derived from the deterministic (fixed-seed) run — see
/// the `lease_manager_handover_*` tests, which pin the milestones it targets.
pub fn lease_manager_handover() -> Scenario {
    let lease = |g, h| LeaseId {
        grantor: g,
        grantee: h,
    };
    Scenario::new(4)
        .seed(0x0432_0432) // fixed: deterministic, reproducible run
        .duration(20_000)
        .lease(0, 1)
        .lease(0, 2)
        .lease(0, 3)
        // Grant to 1 and 2 up front.
        .command(300, Command::Initiate(lease(0, 1)))
        .command(300, Command::Initiate(lease(0, 2)))
        // After two renew rounds with node 1 (~t=1900), hand its slot to 3: revoke
        // 1 (2 keeps renewing, untouched). Guarding 3 waits for 1's revoke ack, so
        // it's a second scripted step at the tick that ack lands (~t=2500).
        .command(1900, Command::Revoke(lease(0, 1)))
        .command(2500, Command::Initiate(lease(0, 3)))
}

/// Classic all-to-one leader leases with 5 nodes: every follower (1..5) grants to
/// the leader (node 0), which counts its own implicit self-grant. Each `(g → 0)`
/// lease is an independent one-to-one primitive; once the leader holds a majority
/// it is the stable leader (can read locally). All four guards are scripted at a
/// known tick so establishment is deterministic (not stochastic per-poll), then
/// the leader is renewed by all, steadily, on a reliable link.
pub fn leader_leases() -> Scenario {
    let s = Scenario::new(5)
        .seed(0x0510_0510) // fixed: deterministic, reproducible run
        .duration(20_000)
        .all_to_one(0);
    // Guard all four follower→leader leases up front, so the leader's majority
    // establishes at a stable time.
    guard_all(s, 300)
}

/// The two quorum-lease holders (grantees) shared by the quorum demos.
const QUORUM_HOLDERS: [usize; 2] = [0, 2];

/// A 5-node all-to-many quorum-lease scenario with holders [`QUORUM_HOLDERS`],
/// each granted by every other node, and every grantor→holder guard scripted at
/// t=300 so both holders' majorities establish deterministically. `seed` picks the
/// reproducible run; the caller layers on write configuration.
fn quorum_base(seed: u64) -> Scenario {
    let s = Scenario::new(5)
        .seed(seed)
        .duration(20_000)
        .all_to_many(&QUORUM_HOLDERS);
    guard_all(s, 300)
}

/// All-to-many quorum read leases with 5 nodes: two holders (grantees) 0 and 2,
/// each granted by every other node (`all_to_many(&[0, 2])`). No writes — the
/// steady read-lease state, before any write disruption. Each `(g → h)` lease is
/// an independent one-to-one primitive; a holder reads locally once a majority
/// grants it. All eight guards are scripted up front so establishment is
/// deterministic, then both holders are renewed by all, steadily.
pub fn quorum_leases() -> Scenario {
    quorum_base(0x0520_0520)
}

/// Quorum read leases *disrupted by a write*: the same two-holder setup as
/// [`quorum_leases`], but a single **disruptive** write is served by the leader
/// (smallest-id holder, node 0) at t=2000. The write is the revocation — every
/// node tears down the leases it takes part in (reads it holds *and* grants it
/// makes) and freezes until the commit, then re-establishes from scratch,
/// re-guarding. Illustrates the quorum-lease write coupling: a write to a leased
/// object suspends local reads and forces a fresh guard round before they resume.
pub fn quorum_leases_write_disruption() -> Scenario {
    quorum_base(0x0521_0521)
        // No periodic cadence; a single scripted disruptive write at t=2000, by
        // which point both holders have established and renewed a few rounds. On
        // its commit every torn-down grant re-guards deterministically (commit-
        // driven recovery — no `initiate_chance` needed), so the whole run is
        // reproducible.
        .writes(None, true)
        .command(2000, Command::Write)
}

/// All-to-all roster leases with 5 nodes: every ordered distinct pair gets a lease
/// (`all_to_all` → 20 leases), so each node is both grantor to and grantee of all
/// four peers. The lease says only "we agree on the roster"; a node holds a stable
/// roster once a majority grant it. Every grantor→grantee guard is scripted up
/// front so the full mesh establishes deterministically, then all 20 leases renew
/// steadily on a reliable link.
pub fn roster_leases() -> Scenario {
    let s = Scenario::new(5)
        .seed(0x0530_0530) // fixed: deterministic, reproducible run
        .duration(20_000)
        .all_to_all();
    // Guard every ordered pair up front, so the whole mesh establishes at a stable
    // time rather than via staggered stochastic initiation.
    guard_all(s, 300)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Time;
    use crate::engine::Engine;
    use crate::event::{EventKind, LeaseStatus, MsgKind};

    /// Grantee's held status for lease `grantor -> grantee` at the engine's
    /// current frame time.
    fn grantee_status(eng: &Engine, t: crate::clock::Time) -> LeaseStatus {
        eng.frame_at(t).leases[0].grantee_status
    }

    #[test]
    fn one_to_one_success_establishes_and_stays_active() {
        let mut eng = Engine::new(one_to_one_success());

        // Before the scripted initiate the lease is idle.
        eng.advance_to(200);
        assert_eq!(grantee_status(&eng, 200), LeaseStatus::Inactive);

        // It establishes within a couple of round-trips of the t=300 guard.
        eng.advance_to(1500);
        assert_eq!(grantee_status(&eng, 1500), LeaseStatus::Active);

        // And, with no message loss, it stays active through steady renewals —
        // including the tick the walkthrough canvas stops at (13_000) and beyond.
        for t in [3000, 6000, 12000, 13000, 19000] {
            eng.advance_to(t);
            assert_eq!(
                grantee_status(&eng, t),
                LeaseStatus::Active,
                "lease should still be held at t={t}"
            );
        }
    }

    #[test]
    fn one_to_one_success_grantor_stays_active_too() {
        // The grantor side (what colors the OUT bar) must also hold: no stale-
        // renew give-up on a perfectly reliable link.
        let mut eng = Engine::new(one_to_one_success());
        for t in [1500, 6000, 13000] {
            eng.advance_to(t);
            assert_eq!(
                eng.frame_at(t).leases[0].grantor_status,
                LeaseStatus::Active,
                "grantor should still grant at t={t}"
            );
        }
    }

    #[test]
    fn one_to_one_success_reaches_third_renew_reply() {
        // The walkthrough canvas stops when the grantor receives its 3rd
        // `RenewReply` (a `MsgKind::RenewReply` delivered *to* node 0). Confirm
        // that event actually occurs within the scenario's duration, the lease is
        // active by then, and it lands comfortably before the stochastic run's end.
        let mut eng = Engine::new(one_to_one_success());
        let events = eng.advance_to(20_000);
        let mut nth_reply_at: Option<Time> = None;
        let mut seen = 0;
        for ev in &events {
            if let EventKind::MessageDelivered {
                to: 0,
                kind: MsgKind::RenewReply,
                ..
            } = ev.kind
            {
                seen += 1;
                if seen == 3 {
                    nth_reply_at = Some(ev.at);
                    break;
                }
            }
        }
        let at = nth_reply_at.expect("grantor should receive a 3rd RenewReply");
        // Renews go out every ~600 ticks after activation (~1000), each replied a
        // round-trip later, so the 3rd reply lands well within the 13k-tick cap.
        assert!(at < 6_000, "3rd RenewReply came late, at t={at}");
        eng.advance_to(at);
        assert_eq!(
            eng.frame_at(at).leases[0].grantee_status,
            LeaseStatus::Active,
            "lease should be active by the 3rd RenewReply"
        );
    }

    #[test]
    fn one_to_one_guard_reply_lost_drops_ack_then_grantor_gives_up() {
        // The grantee acks the guard, but every `GuardReply` is dropped. Confirm:
        // a `GuardReply` from node 1 is actually dropped, and (with no
        // re-initiation) the grantor never activates — it returns to idle after
        // its guard window lapses and stays there.
        let mut eng = Engine::new(one_to_one_guard_reply_lost());
        let events = eng.advance_to(20_000);

        let drop_at = events.iter().find_map(|ev| match ev.kind {
            EventKind::MessageDropped {
                from: 1,
                kind: MsgKind::GuardReply,
                ..
            } => Some(ev.at),
            _ => None,
        });
        let drop_at = drop_at.expect("the grantee's GuardReply should be dropped");
        // The guard goes out at t=300, reaches the grantee a message-delay later,
        // and its ack is dropped on the way back — so the drop lands within a
        // couple of message delays of the guard.
        assert!(drop_at < 1_200, "GuardReply drop came late, at t={drop_at}");

        // Never activates on either side, at any point in the run.
        for t in [drop_at, drop_at + 600, 3_000, 10_000, 20_000] {
            let f = eng.frame_at(t);
            assert_ne!(
                f.leases[0].grantor_status,
                LeaseStatus::Active,
                "grantor must never activate (t={t})"
            );
            assert_ne!(
                f.leases[0].grantee_status,
                LeaseStatus::Active,
                "grantee must never activate (t={t})"
            );
        }

        // And once its guard window lapses the grantor is idle again (not stuck
        // guarding) — a fresh guard could start, but nothing re-initiates it.
        let f = eng.frame_at(3_000);
        assert_ne!(
            f.leases[0].grantor_status,
            LeaseStatus::Guarding,
            "grantor should not stay stuck in the guard phase"
        );
    }

    #[test]
    fn one_to_one_guard_reply_lost_both_guards_expire_grantee_last() {
        // The walkthrough canvas stops "when both guard timers expire" — the later
        // of the grantor's guard-window lapse (no event) and the grantee's
        // guard-window lapse (emits a grantee `Expired`). Confirm the grantee's
        // guard `Expired` event fires, and that by then the grantor is already out
        // of its guard (so this really is the *last* of the two to expire).
        let mut eng = Engine::new(one_to_one_guard_reply_lost());
        let events = eng.advance_to(20_000);

        let grantee_expired_at = events.iter().find_map(|ev| match ev.kind {
            EventKind::GranteeLease {
                status: LeaseStatus::Expired,
                ..
            } => Some(ev.at),
            _ => None,
        });
        let at = grantee_expired_at.expect("grantee's guard window should lapse to Expired");
        // Guard window is `t_guard - t_delta = 1400`; receipt is a message-delay
        // after the t=300 guard, so the lapse lands roughly t≈1.5–2k.
        assert!(
            (1_000..3_000).contains(&at),
            "grantee guard expiry landed oddly, at t={at}"
        );

        // The grantor gave up first: its guard window (`t_guard - t_delta = 1400`)
        // runs from the earlier t=300 send, so it lapses (~t=1700) before the
        // grantee's does — by the grantee's expiry it is no longer guarding, so the
        // grantee is the last to expire.
        let f = eng.frame_at(at);
        assert_ne!(
            f.leases[0].grantor_status,
            LeaseStatus::Guarding,
            "grantor should already be out of its guard by the grantee's expiry"
        );
    }

    #[test]
    fn one_to_one_revoked_establishes_renews_then_revoke_is_acked() {
        // Establishes, exchanges exactly two renews, then the t=1920 revoke drops
        // the grantee's hold and it acks with a `RevokeReply`. Confirm: active
        // before the revoke, exactly two renews sent (no stray 3rd), a grantee
        // `Expired` at the revoke, and a `RevokeReply` delivered back to the grantor.
        // Active and holding just before the scripted revoke (sampled on its own
        // engine — `advance_to` only moves forward, so we can't rewind after the
        // full run below).
        {
            let mut probe = Engine::new(one_to_one_revoked());
            probe.advance_to(1_900);
            assert_eq!(
                probe.frame_at(1_900).leases[0].grantee_status,
                LeaseStatus::Active,
                "lease should be active before the revoke"
            );
        }

        let mut eng = Engine::new(one_to_one_revoked());
        let events = eng.advance_to(20_000);

        // Exactly two renews are sent — the grantor revokes (t=1920) before the
        // 3rd would fall due (~1950), so no stray renew goes out alongside it.
        let renews_sent = events
            .iter()
            .filter(|ev| {
                matches!(
                    ev.kind,
                    EventKind::MessageSent {
                        from: 0,
                        kind: MsgKind::Renew,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(renews_sent, 2, "grantor should send exactly two renews");

        // The revoke ack round-trips back to the grantor.
        let ack_at = events.iter().find_map(|ev| match ev.kind {
            EventKind::MessageDelivered {
                to: 0,
                kind: MsgKind::RevokeReply,
                ..
            } => Some(ev.at),
            _ => None,
        });
        let ack_at = ack_at.expect("grantor should receive a RevokeReply");
        assert!(
            (2_000..3_000).contains(&ack_at),
            "revoke ack landed oddly, at t={ack_at}"
        );
        // The grantee dropped its hold once revoked.
        assert_eq!(
            eng.frame_at(ack_at).leases[0].grantee_status,
            LeaseStatus::Expired,
            "grantee should have dropped its hold by the revoke ack"
        );
        // On the ack the grantor leaves the granting state (rather than waiting for
        // its padded `D'` to lapse) — so its OUT timer bar grays out at that moment.
        let grantor_expired_at = events.iter().find_map(|ev| match ev.kind {
            EventKind::GrantorLease {
                status: LeaseStatus::Expired,
                ..
            } => Some(ev.at),
            _ => None,
        });
        assert_eq!(
            grantor_expired_at,
            Some(ack_at),
            "grantor should expire exactly when the revoke ack arrives"
        );
    }

    #[test]
    fn one_to_one_renew_replies_lost_both_sides_expire() {
        // Establishes and renews once, then all renew acks are dropped from t=1300.
        // The grantor stops hearing back and eventually both sides expire. Confirm
        // exactly one grantor `Expired` and one grantee `Expired` fire, and capture
        // the later of the two (when "both timers have expired").
        // It did establish first (one renew reply got through before t=1300) —
        // sampled on its own engine since `advance_to` only moves forward.
        {
            let mut probe = Engine::new(one_to_one_renew_replies_lost());
            probe.advance_to(1_200);
            assert_eq!(
                probe.frame_at(1_200).leases[0].grantee_status,
                LeaseStatus::Active,
                "lease should have established before renew acks go silent"
            );
        }

        let mut eng = Engine::new(one_to_one_renew_replies_lost());
        let events = eng.advance_to(20_000);

        let grantor_expired = events.iter().find_map(|ev| match ev.kind {
            EventKind::GrantorLease {
                status: LeaseStatus::Expired,
                ..
            } => Some(ev.at),
            _ => None,
        });
        let grantee_expired = events.iter().find_map(|ev| match ev.kind {
            EventKind::GranteeLease {
                status: LeaseStatus::Expired,
                ..
            } => Some(ev.at),
            _ => None,
        });
        let g0 = grantor_expired.expect("grantor should expire when acks go silent");
        let g1 = grantee_expired.expect("grantee should expire when renews stop");
        // Both land within a few lease lifetimes; the run captures the later one.
        let both = g0.max(g1);
        assert!(both < 8_000, "both-expired landed late, at t={both}");
        assert_ne!(
            eng.frame_at(both).leases[0].grantee_status,
            LeaseStatus::Active
        );
        assert_ne!(
            eng.frame_at(both).leases[0].grantor_status,
            LeaseStatus::Active
        );
    }

    #[test]
    fn one_to_one_renew_replies_lost_grantor_stops_after_unacked_renew() {
        // One renew in flight: the grantor sends a renew only after the previous
        // one is acked. Here the first renew is acked but every reply after that is
        // dropped, so the grantor sends *exactly one* un-acked renew and then goes
        // silent (rather than streaming renews on a fixed cadence until the
        // reply-timeout). Count the renews sent after the last delivered reply.
        let mut eng = Engine::new(one_to_one_renew_replies_lost());
        let events = eng.advance_to(20_000);

        let last_reply_at = events
            .iter()
            .filter_map(|ev| match ev.kind {
                EventKind::MessageDelivered {
                    to: 0,
                    kind: MsgKind::RenewReply,
                    ..
                } => Some(ev.at),
                _ => None,
            })
            .next_back()
            .expect("at least one renew reply gets through");

        let renews_after_last_reply = events
            .iter()
            .filter(|ev| {
                matches!(
                    ev.kind,
                    EventKind::MessageSent {
                        from: 0,
                        kind: MsgKind::Renew,
                        ..
                    }
                ) && ev.at > last_reply_at
            })
            .count();
        assert_eq!(
            renews_after_last_reply, 1,
            "grantor should send exactly one un-acked renew, then stop (sent {renews_after_last_reply})"
        );
    }

    #[test]
    fn one_to_one_renew_replies_lost_grantor_bar_tracks_expiry() {
        // Regression: the grantor's countdown bar reads its real `D'`
        // (`grant_expiry`) normalized by the span `D'` is provisioned over, so it
        // is *always draining* — never pegged flat-full — and reaches 0 exactly at
        // expiry. This guards two past bugs: (a) a stale separate display timer that
        // emptied thousands of ticks before gray-out, and (b) normalizing the
        // over-provisioned `D'` by only `t_lease`, which pegged the bar at 1.0 until
        // `local` came within a span of `D'`.
        let grantor_expired_at = {
            let mut e = Engine::new(one_to_one_renew_replies_lost());
            e.advance_to(20_000)
                .iter()
                .find_map(|ev| match ev.kind {
                    EventKind::GrantorLease {
                        status: LeaseStatus::Expired,
                        ..
                    } => Some(ev.at),
                    _ => None,
                })
                .expect("grantor expires")
        };

        let sample = |t: crate::clock::Time| {
            let mut e = Engine::new(one_to_one_renew_replies_lost());
            e.advance_to(t);
            e.frame_at(t).leases[0].grantor_fill
        };

        // Never pegged flat-full while healthy: two samples a little apart within
        // the same renew interval must differ (the bar is draining, not clamped).
        // Both are past the last confirming renew reply, so nothing re-arms between.
        let a = sample(4_000);
        let b = sample(4_250);
        assert!(
            a < 1.0 && b < a,
            "bar should be draining, not pegged: {a} -> {b}"
        );

        // Drain lines up with the real `D'` gray-out: still clearly filled a lease
        // span before expiry (the bar drains over the ~`t_guard + t_lease + 2·t_delta`
        // grant span, so a `t_lease` before the end it's roughly a third full), and
        // near-empty just before it — not bottomed out thousands of ticks early.
        assert!(
            sample(grantor_expired_at - 1_400) > 0.3,
            "bar should still be clearly filled one span before expiry"
        );
        assert!(
            sample(grantor_expired_at - 50) < 0.1,
            "bar should be nearly empty just before expiry"
        );
    }

    /// Grantee's held status for lease `0 -> h` at the engine's current frame.
    fn held_status(eng: &Engine, h: usize, t: Time) -> LeaseStatus {
        let f = eng.frame_at(t);
        f.leases
            .iter()
            .find(|b| b.grantor == 0 && b.grantee == h)
            .expect("lease 0->h declared")
            .grantee_status
    }

    #[test]
    fn lease_manager_handover_scripts_hit_their_milestones() {
        // The scripted ticks depend on the run's timing, so pin the two data-driven
        // ones: the revoke of 1 must land *after* node 1's 2nd renew reply, and the
        // guard of 3 must land *after* node 1's revoke ack reaches the grantor.
        let mut eng = Engine::new(lease_manager_handover());
        let events = eng.advance_to(20_000);

        // Node 1's renew replies delivered to the grantor, in order.
        let n1_renew_replies: Vec<Time> = events
            .iter()
            .filter_map(|ev| match ev.kind {
                EventKind::MessageDelivered {
                    from: 1,
                    to: 0,
                    kind: MsgKind::RenewReply,
                } => Some(ev.at),
                _ => None,
            })
            .collect();
        assert!(
            n1_renew_replies.len() >= 2,
            "node 1 should exchange >=2 renews before the handover"
        );
        // The Revoke(0->1) is scripted at t=1900; it must follow the 2nd reply.
        assert!(
            n1_renew_replies[1] < 1_900,
            "revoke should fire after node 1's 2nd renew reply (2nd at {})",
            n1_renew_replies[1]
        );

        // Node 1's revoke ack reaches the grantor, and the guard of 3 (t=2500)
        // follows it.
        let revoke_ack_at = events
            .iter()
            .find_map(|ev| match ev.kind {
                EventKind::MessageDelivered {
                    from: 1,
                    to: 0,
                    kind: MsgKind::RevokeReply,
                } => Some(ev.at),
                _ => None,
            })
            .expect("grantor should receive node 1's revoke ack");
        assert!(
            revoke_ack_at < 2_500,
            "guarding 3 should start after 1's revoke ack (ack at {revoke_ack_at})"
        );
        let guard3_at = events
            .iter()
            .find_map(|ev| match ev.kind {
                EventKind::MessageSent {
                    from: 0,
                    to: 3,
                    kind: MsgKind::Guard,
                    ..
                } => Some(ev.at),
                _ => None,
            })
            .expect("grantor should guard node 3");
        assert!(guard3_at >= revoke_ack_at, "guard 3 after the revoke ack");
    }

    #[test]
    fn lease_manager_handover_two_holds_then_one_two_three() {
        // Snapshot the three grantees' held status at key moments (fresh engines,
        // since `advance_to` only moves forward):
        let held = |h: usize, t: Time| {
            let mut e = Engine::new(lease_manager_handover());
            e.advance_to(t);
            held_status(&e, h, t)
        };

        // Early: 1 and 2 hold, 3 is idle.
        assert_eq!(held(1, 1_500), LeaseStatus::Active, "1 held early");
        assert_eq!(held(2, 1_500), LeaseStatus::Active, "2 held early");
        assert_eq!(held(3, 1_500), LeaseStatus::Inactive, "3 idle early");

        // After the handover settles: 1 dropped, 2 still held (never interrupted),
        // 3 now held.
        assert_ne!(
            held(1, 3_500),
            LeaseStatus::Active,
            "1 dropped after handover"
        );
        assert_eq!(
            held(2, 3_500),
            LeaseStatus::Active,
            "2 rides on, still held"
        );
        assert_eq!(
            held(3, 3_500),
            LeaseStatus::Active,
            "3 established after handover"
        );
    }

    #[test]
    fn lease_manager_handover_node2_never_interrupted() {
        // Node 2's grant is never revoked, so it stays continuously held from the
        // moment it activates through the whole handover — no gap.
        // Sampled densely across the handover window on fresh engines.
        for t in (1_000..=4_000).step_by(250) {
            let mut e = Engine::new(lease_manager_handover());
            e.advance_to(t);
            assert_eq!(
                held_status(&e, 2, t),
                LeaseStatus::Active,
                "node 2 should stay held throughout the handover (t={t})"
            );
        }
    }

    #[test]
    fn leader_leases_all_followers_grant_the_leader() {
        // 5 nodes, all-to-one: each follower's `(g -> 0)` grant should activate,
        // so the leader holds all four — a majority (3 of 5, counting itself).
        let mut eng = Engine::new(leader_leases());
        eng.advance_to(1_500);
        let f = eng.frame_at(1_500);
        let held = f
            .leases
            .iter()
            .filter(|b| b.grantee == 0 && b.grantee_status == LeaseStatus::Active)
            .count();
        assert_eq!(held, 4, "leader should hold all four follower grants");
    }

    #[test]
    fn leader_leases_every_follower_renews_at_least_thrice() {
        // The canvas stops on the 12th `Renew` to reach the leader (4 grantors × 3).
        // Confirm that milestone lands where expected and that by it every follower
        // has renewed the leader at least three times.
        use std::collections::BTreeMap;
        let mut eng = Engine::new(leader_leases());
        let events = eng.advance_to(20_000);

        let mut per_follower: BTreeMap<usize, usize> = BTreeMap::new();
        let mut twelfth_at = None;
        let mut total = 0;
        for ev in &events {
            if let EventKind::MessageDelivered {
                from,
                to: 0,
                kind: MsgKind::Renew,
            } = ev.kind
            {
                *per_follower.entry(from).or_default() += 1;
                total += 1;
                if total == 12 {
                    twelfth_at = Some(ev.at);
                    break;
                }
            }
        }
        let at = twelfth_at.expect("leader should receive a 12th renew");
        assert!(at < 4_000, "12th renew to leader came late, at t={at}");
        // Every one of the 4 followers has renewed the leader >= 3 times by then.
        assert_eq!(per_follower.len(), 4, "all four followers renew the leader");
        assert!(
            per_follower.values().all(|&c| c >= 3),
            "every follower should have renewed the leader >=3 times: {per_follower:?}"
        );
    }

    #[test]
    fn quorum_leases_both_holders_establish_a_majority() {
        // Two holders (0 and 2), each granted by the other four → each holds all
        // four grants (a majority of 5, counting itself).
        let mut eng = Engine::new(quorum_leases());
        eng.advance_to(1_500);
        let f = eng.frame_at(1_500);
        for h in [0usize, 2] {
            let held = f
                .leases
                .iter()
                .filter(|b| b.grantee == h && b.grantee_status == LeaseStatus::Active)
                .count();
            assert_eq!(held, 4, "holder {h} should hold all four grants");
        }
    }

    #[test]
    fn quorum_leases_both_holders_renew_thrice_by_24th() {
        // The canvas stops on the 24th `Renew` to reach a holder (2 holders × 4
        // grantors × 3 rounds). By then *both* holders must have been renewed >= 3
        // times by every one of their four grantors.
        use std::collections::BTreeMap;
        let mut eng = Engine::new(quorum_leases());
        let events = eng.advance_to(20_000);

        // per holder -> per grantor renew count, tallied up to the 24th overall.
        let mut per_holder: BTreeMap<usize, BTreeMap<usize, usize>> = BTreeMap::new();
        let mut total = 0;
        let mut twenty_fourth_at = None;
        for ev in &events {
            if let EventKind::MessageDelivered {
                from,
                to,
                kind: MsgKind::Renew,
            } = ev.kind
                && (to == 0 || to == 2)
            {
                *per_holder.entry(to).or_default().entry(from).or_default() += 1;
                total += 1;
                if total == 24 {
                    twenty_fourth_at = Some(ev.at);
                    break;
                }
            }
        }
        let at = twenty_fourth_at.expect("holders should receive a 24th renew");
        assert!(at < 4_000, "24th renew to a holder came late, at t={at}");
        for h in [0usize, 2] {
            let per = per_holder.get(&h).expect("holder renewed");
            assert_eq!(per.len(), 4, "holder {h} renewed by all four grantors");
            assert!(
                per.values().all(|&c| c >= 3),
                "holder {h} should be renewed >=3 times by each grantor: {per:?}"
            );
        }
    }

    #[test]
    fn quorum_write_disruption_tears_down_then_re_establishes() {
        // The write at t=2000 tears every held lease down (both holders lose all
        // their grants), then the cluster re-establishes: by well after the commit
        // both holders again hold all four grants.
        // Before the write: both holders hold their majority (sampled on its own
        // engine — advance_to only moves forward).
        let held_at = |t: crate::clock::Time, h: usize| {
            let mut e = Engine::new(quorum_leases_write_disruption());
            e.advance_to(t);
            e.frame_at(t)
                .leases
                .iter()
                .filter(|b| b.grantee == h && b.grantee_status == LeaseStatus::Active)
                .count()
        };
        assert_eq!(
            held_at(1_900, 0),
            4,
            "holder 0 established before the write"
        );
        assert_eq!(
            held_at(1_900, 2),
            4,
            "holder 2 established before the write"
        );

        // Once the write has swept the cluster (its `Write`s delivered ~t=2200),
        // both holders are torn down (0 grants held).
        assert_eq!(held_at(2_300, 0), 0, "holder 0 suspended by the write");
        assert_eq!(held_at(2_300, 2), 0, "holder 2 suspended by the write");

        // Well after the commit, both holders have re-established their majority.
        assert_eq!(held_at(6_000, 0), 4, "holder 0 re-established after commit");
        assert_eq!(held_at(6_000, 2), 4, "holder 2 re-established after commit");
    }

    #[test]
    fn quorum_write_disruption_recovery_is_commit_driven_via_guard() {
        // Recovery is deterministic and guard-based: after the commit, fresh
        // `Guard` messages re-open every torn-down grant (no reliance on stochastic
        // re-initiation). Confirm guards fire *after* the commit and both holders
        // then renew three full rounds again.
        use std::collections::BTreeMap;
        let mut eng = Engine::new(quorum_leases_write_disruption());
        let events = eng.advance_to(14_000);

        let commit_at = events
            .iter()
            .find_map(|ev| match ev.kind {
                EventKind::WriteCommitted { .. } => Some(ev.at),
                _ => None,
            })
            .expect("the write commits");
        // `>=` because the leader commits locally and re-guards its own grant in
        // the same tick as the commit; peers re-guard on the `Commit` they receive.
        let guards_at_or_after_commit = events
            .iter()
            .filter(|ev| {
                matches!(
                    ev.kind,
                    EventKind::MessageSent {
                        kind: MsgKind::Guard,
                        ..
                    }
                ) && ev.at >= commit_at
            })
            .count();
        assert!(
            guards_at_or_after_commit >= 8,
            "every torn-down grant should re-guard after the commit (saw {guards_at_or_after_commit})"
        );

        // Both holders complete 3 post-commit renew rounds from all four grantors.
        let mut post: BTreeMap<usize, BTreeMap<usize, usize>> = BTreeMap::new();
        let mut committed = false;
        let mut milestone = None;
        for ev in &events {
            match ev.kind {
                EventKind::WriteCommitted { .. } => committed = true,
                EventKind::MessageDelivered {
                    from,
                    to,
                    kind: MsgKind::Renew,
                } if committed && (to == 0 || to == 2) => {
                    *post.entry(to).or_default().entry(from).or_default() += 1;
                    let done = [0usize, 2].iter().all(|h| {
                        post.get(h)
                            .is_some_and(|m| m.len() == 4 && m.values().all(|&c| c >= 3))
                    });
                    if done && milestone.is_none() {
                        milestone = Some(ev.at);
                    }
                }
                _ => {}
            }
        }
        let at = milestone.expect("both holders should re-renew 3 rounds post-commit");
        assert!(
            at < 8_000,
            "post-commit 3-round milestone came late, at t={at}"
        );
    }

    #[test]
    fn roster_leases_full_mesh_renews_three_rounds() {
        // 5 nodes all-to-all → 20 leases. The canvas stops on the 60th `Renew`
        // delivered (20 leases × 3 rounds). Confirm all 20 leases exist, every one
        // has renewed >= 3 times by that milestone, and it lands where expected.
        use std::collections::BTreeMap;
        let mut eng = Engine::new(roster_leases());
        let events = eng.advance_to(20_000);

        let mut per_lease: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        let mut total = 0;
        let mut sixtieth_at = None;
        for ev in &events {
            if let EventKind::MessageDelivered {
                from,
                to,
                kind: MsgKind::Renew,
            } = ev.kind
            {
                *per_lease.entry((from, to)).or_default() += 1;
                total += 1;
                if total == 60 {
                    sixtieth_at = Some(ev.at);
                    break;
                }
            }
        }
        let at = sixtieth_at.expect("mesh should deliver a 60th renew");
        assert!(at < 4_000, "60th renew came late, at t={at}");
        assert_eq!(per_lease.len(), 20, "all 20 ordered-pair leases renew");
        assert!(
            per_lease.values().all(|&c| c >= 3),
            "every lease should have renewed >=3 times: {per_lease:?}"
        );
    }

    #[test]
    fn roster_leases_every_node_holds_a_majority() {
        // Each node is a grantee of the other four, so once the mesh establishes it
        // holds all four grants — a majority (3 of 5, counting itself) → a stable
        // roster at every node.
        let mut eng = Engine::new(roster_leases());
        eng.advance_to(1_500);
        let f = eng.frame_at(1_500);
        for node in 0..5 {
            let held = f
                .leases
                .iter()
                .filter(|b| b.grantee == node && b.grantee_status == LeaseStatus::Active)
                .count();
            assert_eq!(held, 4, "node {node} should hold all four grants");
        }
    }
}
