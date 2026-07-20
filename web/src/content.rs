//! Walkthrough content, authored in `content/*.md` and rendered to HTML at build
//! time (see `build.rs`). This module only declares the data shapes; the actual
//! `SECTIONS` literal is generated into `$OUT_DIR/content_gen.rs`. The `Home`
//! component in `components.rs` templates these into the page.

/// One walkthrough section: its chrome (from front-matter) plus an ordered list
/// of prose/widget blocks (from the markdown body).
#[derive(PartialEq)]
pub struct Section {
    /// Anchor id and `#…` link target.
    pub id: &'static str,
    /// `"intro"` (plain `Section`) or `"algo"` (numbered `AlgoSection`).
    pub kind: &'static str,
    /// Ordinal (`"01"`–`"06"`); empty for the intro.
    pub step: &'static str,
    /// Small-caps pattern tag (`"one-to-one"` … `"all-to-all"`); empty for intro.
    pub pattern: &'static str,
    /// Section heading.
    pub title: &'static str,
    /// The `(pro, con)` pair for the section's `Tradeoff`, if any.
    pub tradeoff: Option<(&'static str, &'static str)>,
    /// Lead-in paragraph (HTML) preceding the recap table; empty if none.
    pub recap_lead: &'static str,
    /// Rows of the closing recap table (roster section only).
    pub recap: &'static [RecapRow],
    /// Prose and widget blocks in document order.
    pub blocks: &'static [Block],
}

/// A row of the recap table: an accumulated trait and the rung it first appeared
/// on. `new` marks the trait Bodega itself contributes (highlighted).
#[derive(PartialEq)]
pub struct RecapRow {
    pub label: &'static str,
    pub seen: &'static str,
    pub new: bool,
}

/// An ordered piece of a section body: pre-rendered prose HTML, or a widget
/// placed within the prose flow (rendered by the component from section fields).
#[derive(PartialEq)]
pub enum Block {
    /// Pre-rendered markdown prose, injected via `dangerous_inner_html`.
    Html(&'static str),
    /// A simulation canvas. The string names a hardcoded scenario (see
    /// `crate::scenarios`); empty renders the plain placeholder.
    Figure(&'static str),
    /// The `Tradeoff` pro/con pair (uses `Section::tradeoff`).
    Tradeoff,
    /// The recap lead-in + table (uses `Section::recap_lead` / `recap`).
    Recap,
}

include!(concat!(env!("OUT_DIR"), "/content_gen.rs"));
