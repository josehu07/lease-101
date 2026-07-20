//! Page components. A single-page walkthrough composed of stacked sections.
//!
//! The walkthrough prose is authored in `content/*.md` and rendered to HTML at
//! build time (see `crate::content` and `build.rs`); the components here are
//! thin templates that slot that content into the page chrome.

use dioxus::prelude::*;

use crate::content::{self, Block, Section as SectionData};

/// Base browser-tab title (matches `Dioxus.toml`'s `web.app.title`), used on the
/// home route. Routes set the tab title via `document::Title`; `/sim` uses its
/// own standalone "Lease Sim Playground".
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
/// then sections climb the progression — one-to-one → lease manager → leader →
/// quorum → roster — each motivated by the limitation of the one before it.
/// Every section's prose lives in `content/*.md`; here we just template it.
#[component]
fn Home() -> Element {
    rsx! {
        document::Title { "{BASE_TITLE}" }
        main { id: "top", class: "page",
            for section in content::SECTIONS.iter() {
                WalkthroughSection { section }
            }
            Footer {}
        }
    }
}

/// Render one authored section: the chrome (intro heading, or the algo step +
/// pattern head) around its ordered prose/widget blocks.
#[component]
fn WalkthroughSection(section: &'static SectionData) -> Element {
    let body = rsx! {
        for (i , block) in section.blocks.iter().enumerate() {
            SectionBlock { key: "{i}", section, block }
        }
    };
    rsx! {
        if section.kind == "algo" {
            section { id: section.id, class: "section algo",
                div { class: "algo-head",
                    span { class: "algo-step", "{section.step}" }
                    div {
                        span { class: "algo-pattern", "{section.pattern}" }
                        h2 { "{section.title}" }
                    }
                }
                {body}
            }
        } else {
            section { id: section.id, class: "section",
                h2 { "{section.title}" }
                {body}
            }
        }
    }
}

/// One block within a section: pre-rendered prose HTML, or a widget slotted in
/// from the section's fields (figure / tradeoff / recap).
#[component]
fn SectionBlock(section: &'static SectionData, block: &'static Block) -> Element {
    match block {
        Block::Html(html) => rsx! {
            div { class: "prose", dangerous_inner_html: "{html}" }
        },
        Block::Figure(name) => rsx! {
            crate::scenarios::ScenarioCanvas { name: name.to_string() }
        },
        Block::Tradeoff => match section.tradeoff {
            Some((pro, con)) => rsx! {
                Tradeoff { pro, con }
            },
            None => rsx! {},
        },
        Block::Recap => rsx! {
            if !section.recap_lead.is_empty() {
                p { class: "recap-lead", "{section.recap_lead}" }
            }
            RecapTable { rows: section.recap }
        },
    }
}

/// Sim: a standalone simulator playground with scenario-setup controls over a
/// live `lease_sim`-driven canvas (hosts `Playground`).
#[component]
fn Sim() -> Element {
    rsx! {
        document::Title { "Lease Sim Playground" }
        main { id: "top", class: "page",
            section { class: "section pg-section",
                h2 { "Distributed Lease Simulator Playground" }
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
            "); Plain blog post version of the walkthrough is also available "
            a {
                href: "https://www.josehu.com/technical/2026/07/07/distributed-lease-and-consensus.html",
                target: "_blank",
                rel: "noopener noreferrer",
                "here"
            }
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
                        "📄 Paper"
                    }
                    a {
                        href: "https://github.com/josehu07/summerset/tree/main/tla%2B/bodega",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "📐 TLA"
                        sup { "+" }
                    }
                    a {
                        href: "https://github.com/josehu07/summerset",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "🔅 Summerset"
                    }
                    a {
                        href: "https://github.com/josehu07/lease-101",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "🌐 Web"
                    }
                    a {
                        href: "/sim",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "🕹️ Sim*"
                    }
                }
            }
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

/// The final recap: how each trait accumulated across the progression lands in
/// Bodega. A responsive table mirroring the comparison in `algorithm.md`; rows
/// come from the roster section's front-matter.
#[component]
fn RecapTable(rows: &'static [content::RecapRow]) -> Element {
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
                    for row in rows.iter() {
                        tr {
                            td { "{row.label}" }
                            td { class: if row.new { "recap-new" } else { "" }, "{row.seen}" }
                        }
                    }
                }
            }
        }
    }
}
