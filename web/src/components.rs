//! Page components. A single-page walkthrough composed of stacked sections.

use dioxus::prelude::*;

/// Base browser-tab title (matches `Dioxus.toml`'s `web.app.title`). Routes set
/// this via `document::Title` so navigating between them updates the tab; `/sim`
/// appends a playground suffix.
const BASE_TITLE: &str = "Bodega Consensus";

/// Client-side routes. The home walkthrough and a standalone simulator page,
/// both sharing the sticky `Nav` banner via the `Shell` layout.
#[derive(Routable, Clone, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
        #[route("/")]
        Home {},
        #[route("/sim")]
        Sim {},
}

/// Shared layout: the sticky banner over whichever route is active.
#[component]
fn Shell() -> Element {
    rsx! {
        Nav {}
        Outlet::<Route> {}
    }
}

/// Home: the single-page walkthrough. An intro establishes the lease primitive,
/// then four sections climb the progression — one-to-one → leader → quorum →
/// roster — each motivated by the limitation of the one before it.
#[component]
fn Home() -> Element {
    rsx! {
        document::Title { "{BASE_TITLE}" }
        main { id: "top", class: "page",

            Section { id: "intro", title: "Distributed lease",
                p { class: "lead",
                    "A "
                    em { "lease" }
                    " is a directional, time-bounded promise: one node — the "
                    em { "grantor" }
                    " — promises another — the "
                    em { "grantee" }
                    " — to withhold a conflicting action while the grantee might still "
                    "believe the promise holds."
                }
                p {
                    "The action is whatever the promise covers: granting the same thing "
                    "twice, voting for another leader, changing a value without warning. "
                    "This page follows that one idea as it scales up — until replicas answer "
                    "reads "
                    em { "locally" }
                    ", skipping the quorum round-trip, with no loss of consistency."
                }
                p { "Each rung is driven by a limit of the one below it:" }
                ol { class: "ladder",
                    li {
                        a { href: "#one-to-one", strong { "One-to-one" } }
                        " — the primitive: a safe promise between two nodes."
                    }
                    li {
                        a { href: "#leader", strong { "All-to-one" } }
                        " — everyone leases the leader, so the leader reads locally."
                    }
                    li {
                        a { href: "#quorum", strong { "All-to-many" } }
                        " — a chosen subset leases each object and reads it locally."
                    }
                    li {
                        a { href: "#roster", strong { "All-to-all" } }
                        " — Bodega: lease the cluster roster; anyone reads locally, anytime."
                    }
                }
            }

            // ---- Level 1: the primitive ----
            AlgoSection {
                id: "one-to-one",
                step: "01",
                pattern: "one-to-one",
                title: "Standard one-to-one leasing",
                p { class: "blurb",
                    "The base primitive: one "
                    em { "grantor" }
                    " promises one "
                    em { "grantee" }
                    ". Everything that follows is just this, run at scale."
                }
                p {
                    "Leases need only "
                    em { "bounded drift" }
                    ", never synchronized clocks. Each node checks expiry against its own "
                    "clock, never comparing timestamps — so a constant skew cancels out. Only "
                    "drift "
                    em { "within" }
                    " one period matters, and a small budget "
                    span { class: "var", "t", sub { "Δ" } }
                    " covers it."
                }
                p { "One invariant holds across every renewal:" }
                blockquote { class: "invariant",
                    "The grantor expires the lease no "
                    em { "earlier" }
                    " than the grantee does."
                }
                p {
                    "So the grantee never acts on a lease the grantor has dropped. Asymmetric "
                    "slack (±"
                    span { class: "var", "t", sub { "Δ" } }
                    ") keeps it true: the grantee expires a touch early, the grantor a touch "
                    "late, so the grantor's window always covers the grantee's. Two phases:"
                }
                ol { class: "steps",
                    li {
                        strong { "Guard" }
                        " (once). Tames the "
                        em { "unknown" }
                        " delay of the first message — the grantee accepts the first renewal "
                        "only inside a self-imposed window, letting the grantor bound expiry "
                        "before any reply."
                    }
                    li {
                        strong { "Renew" }
                        " (steady state). The lease starts on the first renewal; a renew loop "
                        "at sub-timeout intervals keeps it fresh, off the critical path — free "
                        "in the common case."
                    }
                }
                p {
                    "To end a lease, the grantor either "
                    em { "revokes" }
                    " it or just withholds renewals and waits out the expiry — the same path a "
                    "failure takes, with no external oracle."
                }
                SimFigure {
                    caption: "One grantor → one grantee: guard handshake, renew loop, and the two expiry timers ticking with their ±t_Δ offset.",
                }
            }

            // ---- Level 2: all-to-one ----
            AlgoSection {
                id: "leader",
                step: "02",
                pattern: "all-to-one",
                title: "Classic all-to-one leader leases",
                p { class: "blurb",
                    "Run the primitive in parallel: every node leases the one node it thinks "
                    "is leader. A majority of those promises becomes a cluster-wide guarantee."
                }
                p {
                    "The primitive is unchanged — only the pattern. Each node grants to the "
                    "latest leader it knows, "
                    em { "first revoking" }
                    " any lease to a previous one. Once a leader holds a majority ("
                    span { class: "var", "⌈n/2⌉" }
                    ", counting its own), it is provably the "
                    em { "only" }
                    " one: two majorities must overlap, and the shared node never promises two "
                    "leaders at once. Quorum intersection, applied to leases."
                }
                p {
                    "The payoff: a "
                    strong { "stable leader" }
                    " knows nothing newer committed without it, so it reads from local state — "
                    "a linearizable read becomes a local clock check, not a network quorum."
                }
                SimFigure {
                    caption: "Five nodes leasing one leader; the leader lights up once its held leases cross the majority line.",
                }
                Tradeoff {
                    pro: "Leases sit off the critical path of consensus; writes never interrupt local reads.",
                    con: "Only leadership is protected — so only the single leader gets local reads.",
                }
            }

            // ---- Level 3: all-to-many ----
            AlgoSection {
                id: "quorum",
                step: "03",
                pattern: "all-to-many",
                title: "All-to-many quorum read leases",
                p { class: "blurb",
                    "One local reader is limiting. Hand the privilege to a "
                    em { "chosen subset" }
                    " instead — picked per object, so each object's frequent readers serve it "
                    "locally."
                }
                p {
                    "A quorum lease is a pair "
                    span { class: "var", "(Q, O)" }
                    ": holder replicas "
                    span { class: "var", "Q" }
                    " over objects "
                    span { class: "var", "O" }
                    ". The promise is new in kind — not \u{201c}you are the leader\u{201d} but "
                    em { "\u{201c}I won't modify these without notifying you first.\u{201d}" }
                    " A holder reads locally once a majority grant it."
                }
                p {
                    "The trick: revocation rides the write path. A Paxos write already needs a "
                    "majority quorum; include the holders in it, and their ordinary "
                    "accept-replies "
                    em { "double as" }
                    " the notification — revocation and write in one round, no extra trip."
                }
                p {
                    "That is also the catch: since the write carries the revocation, writing a "
                    "leased object "
                    em { "suspends" }
                    " local reads and tears the lease down, forcing the holder to re-establish "
                    "it — guard round-trips and all — before reads resume."
                }
                SimFigure {
                    caption: "A subset holding a per-object lease; a write sweeps the quorum and briefly collapses their local-read privilege.",
                }
                Tradeoff {
                    pro: "Local reads expand from one node to a configurable subset, exploiting read locality.",
                    con: "Leases are coupled to the write path, so any write to a leased object disrupts local reads.",
                }
            }

            // ---- Level 4: all-to-all, the culmination ----
            AlgoSection {
                id: "roster",
                step: "04",
                pattern: "all-to-all",
                title: "All-to-all roster leases (Bodega)",
                p { class: "blurb",
                    "The culmination. Decouple "
                    em { "what a lease promises" }
                    " from "
                    em { "who takes part" }
                    ", and lease one piece of cluster metadata instead of leadership or a "
                    "per-object rule."
                }
                p {
                    "That metadata is the "
                    strong { "roster" }
                    ": who is leader "
                    em { "and" }
                    " who the responders (local readers) are per key, tagged by a ballot "
                    span { class: "var", "⟨bal, ros⟩" }
                    ". A lease now says only \u{201c}we agree on the roster.\u{201d} Every node "
                    "is both grantor and grantee. Once its held leases reach a majority, that "
                    "majority agrees on its "
                    strong { "stable roster" }
                    " — and reads may go local."
                }
                p {
                    "Being about metadata, the promise ignores the write log and holds until "
                    "the roster changes. Reads go local "
                    em { "anytime" }
                    ", not just in quiet windows, and the tiny lease messages piggyback on "
                    "existing heartbeats — no common-case overhead."
                }
                p {
                    "One threshold guards safety: a current "
                    em { "view" }
                    " is not a current "
                    em { "log" }
                    ". A just-joined node may lack recent commits, so guards carry the "
                    "grantor's highest accepted slot, and a responder reads locally only after "
                    "catching up to the majority-th such slot — never serving a stale read."
                }
                p {
                    "One disruption strong consistency can't remove is the wait for a value to "
                    "commit. Bodega softens it with "
                    strong { "optimistic holding" }
                    ": a read hitting an accepted-but-uncommitted slot is "
                    em { "parked" }
                    ", not rejected, and answered the instant the commit lands — a brief hold "
                    "instead of a client redirect."
                }
                SimFigure {
                    caption: "A full all-to-all mesh: every node leasing every peer, the roster settling as majorities lock in, undisturbed by the write log.",
                }
                p { class: "recap-lead",
                    "Bodega folds every trait gathered along the climb into one background technique:"
                }
                RecapTable {}
            }

            Footer {}
        }
    }
}

/// Sim: a standalone simulator playground with scenario-setup controls over a
/// live canvas. Wiring it to a running `lease_sim` engine comes next.
#[component]
fn Sim() -> Element {
    rsx! {
        document::Title { "{BASE_TITLE} — Lease Sim Playground" }
        main { id: "top", class: "page",
            section { class: "section pg-section",
                h2 { "Distributed lease simulator playground" }
                crate::playground::Playground {}
            }
            Footer {}
        }
    }
}

/// Small footer at the bottom of the page body (not pinned).
#[component]
fn Footer() -> Element {
    rsx! {
        footer { class: "footer",
            strong { "Author:" }
            " Guanzhou Hu ("
            a {
                href: "https://josehu.com",
                target: "_blank",
                rel: "noopener noreferrer",
                "https://josehu.com"
            }
            "); Plain blog post version is also available "
            a { href: "TODO", target: "_blank", rel: "noopener noreferrer", "here" }
            "."
        }
    }
}

/// Sticky top banner. Right side is reserved for link icons.
#[component]
fn Nav() -> Element {
    rsx! {
        nav { class: "nav",
            div { class: "nav-inner",
                Link { class: "nav-brand", to: Route::Home {}, "Bodega Consensus" }
                div { class: "nav-links",
                    a {
                        href: "https://www.usenix.org/conference/osdi26/presentation/hu-guanzhou",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "Paper"
                    }
                    a {
                        href: "https://github.com/josehu07/summerset/tree/main/tla%2B/bodega_roster_lease",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "TLA"
                        sup { "+" }
                    }
                    a {
                        href: "https://github.com/josehu07/summerset",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "Summerset"
                    }
                    a {
                        href: "https://github.com/josehu07/lease-101",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "Web"
                    }
                    Link { to: Route::Sim {}, "Sim*" }
                }
            }
        }
    }
}

/// A reusable walkthrough section with an anchor id and heading (used for the
/// intro).
#[component]
fn Section(id: String, title: String, children: Element) -> Element {
    rsx! {
        section { id, class: "section",
            h2 { "{title}" }
            {children}
        }
    }
}

/// An algorithm-level section: a numbered step and a pattern tag in the heading
/// (e.g. "all-to-all"), over the level's prose and figure.
#[component]
fn AlgoSection(
    id: String,
    step: &'static str,
    pattern: &'static str,
    title: String,
    children: Element,
) -> Element {
    rsx! {
        section { id, class: "section algo",
            div { class: "algo-head",
                span { class: "algo-step", "{step}" }
                div {
                    span { class: "algo-pattern", "{pattern}" }
                    h2 { "{title}" }
                }
            }
            {children}
        }
    }
}

/// The pro/con pair that motivated the next rung of the ladder.
#[component]
fn Tradeoff(pro: &'static str, con: &'static str) -> Element {
    rsx! {
        div { class: "tradeoff",
            div { class: "tradeoff-pro",
                span { class: "tradeoff-tag", "Pro" }
                span { "{pro}" }
            }
            div { class: "tradeoff-con",
                span { class: "tradeoff-tag", "Con" }
                span { "{con}" }
            }
        }
    }
}

/// A placeholder for a live `lease_sim` animation, with a descriptive caption of
/// what the finished animation will show. Swapped for the real canvas later
/// (see `docs/design/webpage.md`).
#[component]
fn SimFigure(caption: &'static str) -> Element {
    rsx! {
        figure { class: "sim-figure",
            div { class: "sim-placeholder",
                span { "simulation canvas" }
            }
            figcaption { class: "sim-caption", "{caption}" }
        }
    }
}

/// The final recap: how each trait accumulated across the progression lands in
/// Bodega. A responsive table mirroring the comparison in `algorithm.md`.
#[component]
fn RecapTable() -> Element {
    rsx! {
        div { class: "recap",
            table { class: "recap-table",
                thead {
                    tr {
                        th { "Trait" }
                        th { "First seen in" }
                    }
                }
                tbody {
                    tr {
                        td { "Weak clock assumption (bounded drift only)" }
                        td { "one-to-one" }
                    }
                    tr {
                        td { "Fault tolerance via self-healing expiry" }
                        td { "one-to-one" }
                    }
                    tr {
                        td { "Off the critical path (heartbeat piggyback)" }
                        td { "leader leases" }
                    }
                    tr {
                        td { "Configurable set of local readers" }
                        td { "quorum leases" }
                    }
                    tr {
                        td { "Anytime local reads, decoupled from writes" }
                        td { class: "recap-new", "roster leases" }
                    }
                }
            }
        }
    }
}
