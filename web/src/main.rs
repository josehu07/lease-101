//! Dioxus single-page static website for the Distributed Lease 101 walkthrough.
//!
//! Renders entirely client-side (WASM). The lease simulation engine
//! (`lease_sim`) runs live in the browser to drive animations; see
//! `docs/design/webpage.md` for the page design.

use dioxus::prelude::*;

mod components;
mod content;
mod playground;

use components::Route;

/// CSS bundled as a Dioxus asset so the hash-versioned URL is injected for us.
const MAIN_CSS: Asset = asset!("/assets/main.css");
/// Favicon: an all-to-all (K5) leasing mesh, matching the banner's dark blue.
const FAVICON: Asset = asset!("/assets/favicon.svg");

fn main() {
    dioxus::launch(Root);
}

/// Top-level component: wires in global assets, then the page body.
#[component]
fn Root() -> Element {
    rsx! {
        document::Link { rel: "icon", r#type: "image/svg+xml", href: FAVICON }
        document::Stylesheet { href: MAIN_CSS }
        Router::<Route> {}
    }
}
