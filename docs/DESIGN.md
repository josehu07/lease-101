# Project Design

Overall glance of the project is in [README.md](/README.md). Detailed design docs are in the `design/` subfolder, by component.

## Components

- [algorithm.md](design/algorithm.md) — summary of the distributed leasing algorithms
- [simulator.md](design/simulator.md) — the `lease_sim` Rust crate driving live animations
- [webpage.md](design/webpage.md) — the Dioxus single-page site serving the walkthrough

## Plans

- [ ] Condensed summary of distributed leasing algorithms
- [ ] Rust distributed leases simulator code -> wasm
- [ ] Dioxus static webpage serving a concise walkthrough, annotated with simulation widgets
- [ ] Markdown plain blog post version of the walkthrough, with gif figures
- [ ] Reference links to our Bodega paper, the Summerset codebase, TLA+ specs, etc.
- [ ] Scripts for automation and generating everything
- [ ] Verus-based formal verification of algorithm's local invariants
- [ ] Lean formal proof of the leasing algorithm theorem
