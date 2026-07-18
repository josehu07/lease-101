# Guidelines

* NEVER delete files without asking an explicit user question and getting approval
* ALWAYS keep all design and tracker documents under `doc/` up-to-date with the codebase
* SHOULD use `scratch/` as a workspace to hold temporary, fire-and-forget files and experiments
* SHOULD NOT write excessive, wordy doc comments in code; prefer concise explanations

## Rust

* ALWAYS run `cargo clippy --all --all-features` and `cargo +nightly fmt --all` at the end of a session if Rust code edited

## Python

* ALWAYS run `uv run ruff format` at the end of a session if Python code edited
* ALWAYS add complete type annotations to all definition signatures

## Dioxus

* SHOULD NOT launch `dx serve` yourself in the background; user runs it an external terminal and will inspect the webpage outcomes themselves
