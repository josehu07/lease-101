//! Page components. A single-page walkthrough composed of stacked sections.

use dioxus::prelude::*;

/// Metadata for one algorithm walkthrough section.
struct SectionMeta {
    /// Anchor id.
    id: &'static str,
    /// Full section heading.
    title: &'static str,
    /// One-line teaser rendered above the animation.
    blurb: &'static str,
}

/// The four algorithm sections, in the order the walkthrough builds them up.
const ALGO_SECTIONS: &[SectionMeta] = &[
    SectionMeta {
        id: "one-to-one",
        title: "Standard one-to-one leasing",
        blurb: "A directional, time-bounded promise between a single grantor and grantee, kept safe under bounded clock drift.",
    },
    SectionMeta {
        id: "leader",
        title: "Classic all-to-one leader leases",
        blurb: "Every node grants to the one leader; holding a majority of leases makes it the stable leader that reads locally.",
    },
    SectionMeta {
        id: "quorum",
        title: "All-to-many quorum read leases",
        blurb: "A configurable subset of replicas holds per-object leases, so an object's most frequent readers each serve it locally.",
    },
    SectionMeta {
        id: "roster",
        title: "All-to-all roster leases (Bodega)",
        blurb: "All-to-all background leases on a cluster roster, decoupled from writes — any configured node reads locally, anytime.",
    },
];

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

/// Home: the single-page walkthrough — intro, then one section per algorithm.
#[component]
fn Home() -> Element {
    rsx! {
        main { id: "top", class: "page",
            Section { id: "intro", title: "Distributed lease",
                p {
                    "A "
                    em { "lease" }
                    " is a directional, time-bounded promise from a "
                    em { "grantor" }
                    " to a "
                    em { "grantee" }
                    "."
                }
            }
            for meta in ALGO_SECTIONS {
                Section { id: meta.id, title: meta.title,
                    p { class: "blurb", "{meta.blurb}" }
                    SimPlaceholder {}
                }
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

/// A reusable walkthrough section with an anchor id and heading.
#[component]
fn Section(id: String, title: String, children: Element) -> Element {
    rsx! {
        section { id, class: "section",
            h2 { "{title}" }
            {children}
        }
    }
}

/// Placeholder for the WASM-driven simulation canvas, wired up later.
#[component]
fn SimPlaceholder() -> Element {
    rsx! {
        div { class: "sim-placeholder",
            span { "simulation canvas" }
        }
    }
}
