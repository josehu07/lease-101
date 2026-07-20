#!/usr/bin/env python3
"""Assemble the walkthrough into a single plain-markdown blog post.

Concatenates the authored sections (`web/content/*.md`, in reading order) into one
markdown document, mirroring what `build.rs` feeds the website — but for a static
blog. Each `:::figure <name>` sim canvas is replaced by its captured GIF (see
`scripts/gifcap.py`), preceded by the scenario's title as text; the interactive
`:::tradeoff` / `:::recap` widgets are rendered from each section's front-matter as
plain markdown (a pro/con list, a recap table).

The small inline HTML the content uses for math (`<span class="var">…</span>`,
`<sub>`) is converted to plain-markdown equivalents so the output is HTML-free.
A couple of reference footnotes (`FOOTNOTES` / `FOOTNOTE_MARKS`) are anchored to
words in the prose and defined at the document end.

Scenario titles are read straight from `web/src/scenarios.rs` so they never drift
from the site.

Usage:
  uv run scripts/bloggen.py                        # -> docs/blog/walkthrough.md
  uv run scripts/bloggen.py --out path/to/post.md
"""

from __future__ import annotations

import argparse
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONTENT = ROOT / "web/content"
SCENARIOS_RS = ROOT / "web/src/scenarios.rs"
DEFAULT_OUT = ROOT / "docs/blog/walkthrough.md"

# Section files in reading order — the same list `build.rs::FILES` compiles.
FILES = [
    "0-intro",
    "1-one-to-one",
    "2-lease-manager",
    "3-leader-leases",
    "4-quorum-leases",
    "5-roster-leases",
    "6-bodega",
]

# Blog-only heading for the intro section (the site keeps its own `title`).
INTRO_HEADING = "Lease, and What to Expect?"

# GIFs are referenced by this URL pattern — an absolute path on the *blog* site
# where the images are served (`lease-101-` matches `gifcap.py`'s `GIF_PREFIX`).
# They intentionally don't resolve inside this repo; the blog site hosts them.
GIF_URL = "/assets/img/lease-101-{name}.gif"

# In-content links that are site-relative on the live site but must point at the
# deployed site when read from the standalone blog post.
LINK_REWRITES = {"(/sim)": "(https://bodega-consensus.com/sim)"}

# Footnote reference URLs, in order — rendered as `[^n]: [url](url)` at the end.
FOOTNOTES = [
    "https://dl.acm.org/doi/10.1145/2670979.2671001",
    "https://www.usenix.org/conference/osdi26/presentation/hu-guanzhou",
]

# Where each footnote marker anchors: section file -> list of (word, footnote #).
# The `[^n]` marker is inserted right after the first whole-word occurrence of
# `word` in that section's body.
FOOTNOTE_MARKS = {
    "4-quorum-leases": [("locally", 1)],
    "5-roster-leases": [("messages", 2)],
}

FRONT_MATTER = re.compile(r"^\+\+\+\n(.*?)\n\+\+\+\n(.*)$", re.DOTALL)
# `<span class="var">…</span>` math spans, and `<sub>…</sub>` subscripts.
VAR_SPAN = re.compile(r'<span class="var">(.*?)</span>', re.DOTALL)
SUB_TAG = re.compile(r"<sub>(.*?)</sub>", re.DOTALL)
# Same-page anchor links `[text](#frag)` — bolded (the blog host may not support
# fragment jumps). Only `#`-fragment hrefs match, so external/GIF links are safe.
ANCHOR_LINK = re.compile(r"\[([^\]]+)\]\(#[^)]*\)")
# A scenario `lookup` arm: `"name" => Some(ScenarioSpec { title: "…",`.
SPEC_ARM = re.compile(
    r'"(?P<name>[a-z0-9-]+)"\s*=>\s*Some\(ScenarioSpec\s*\{\s*'
    r'title:\s*"(?P<title>[^"]*)"',
    re.DOTALL,
)


def scenario_titles() -> dict[str, str]:
    """Map each wired scenario name to its canvas title, from scenarios.rs."""
    src = SCENARIOS_RS.read_text()
    titles = {m["name"]: m["title"] for m in SPEC_ARM.finditer(src)}
    if not titles:
        raise SystemExit(f"no scenario titles found in {SCENARIOS_RS}")
    return titles


def strip_html(text: str) -> str:
    """Convert the content's tiny inline-HTML surface to plain markdown, and
    rewrite site-relative links to their deployed-site URLs."""
    # Subscripts first (they appear *inside* var spans): `t<sub>Δ</sub>` -> `t_Δ`.
    text = SUB_TAG.sub(lambda m: f"_{m.group(1)}", text)
    # Math variable spans -> inline code, so they still read as symbols.
    text = VAR_SPAN.sub(lambda m: f"`{m.group(1).strip()}`", text)
    # Same-page anchor links -> bold text (no fragment jump on the blog host).
    text = ANCHOR_LINK.sub(lambda m: f"**{m.group(1)}**", text)
    for rel, absolute in LINK_REWRITES.items():
        text = text.replace(rel, absolute)
    return text


def render_tradeoff(fm: dict) -> str:
    """A pro/con pair (from front-matter) as a plain-markdown list."""
    pro = fm.get("tradeoff_pro", "")
    con = fm.get("tradeoff_con", "")
    if not pro:
        return ""
    return f"- **Pro:** {pro}\n- **Con:** {con}\n"


def render_recap(fm: dict) -> str:
    """The recap rows (from front-matter) as a markdown table."""
    rows = fm.get("recap", [])
    if not rows:
        return ""
    lines = ["| Trait | First seen in |", "| --- | --- |"]
    for r in rows:
        seen = r["seen"]
        # Mark the trait Bodega itself contributes (bold), mirroring the site's
        # green-highlighted `recap-new` cell.
        cell = f"**{seen}**" if r.get("new") else seen
        lines.append(f"| {r['trait']} | {cell} |")
    return "\n".join(lines) + "\n"


def render_figure(name: str, titles: dict[str, str]) -> str:
    """A sim canvas: an "Example -- <title>:" caption, then the GIF image."""
    title = titles.get(name, name)
    return f"Example -- {title}:\n\n![{title}]({GIF_URL.format(name=name)})\n"


def mark_footnotes(name: str, text: str) -> str:
    """Insert `[^n]` markers after their anchor words in this section's prose."""
    for word, num in FOOTNOTE_MARKS.get(name, []):
        pat = re.compile(rf"\b{re.escape(word)}\b")
        new, count = pat.subn(f"{word}[^{num}]", text, count=1)
        if not count:
            raise SystemExit(f"{name}.md: footnote anchor word {word!r} not found")
        text = new
    return text


def render_section(name: str, titles: dict[str, str]) -> str:
    """Render one content file to a markdown section."""
    raw = (CONTENT / f"{name}.md").read_text()
    m = FRONT_MATTER.match(raw)
    if not m:
        raise SystemExit(f"{name}.md: missing +++ front-matter")
    fm = tomllib.loads(m.group(1))
    body = mark_footnotes(name, m.group(2))

    # Heading: the intro gets its own blog heading (an H2 "00." so it reads as the
    # zeroth rung); each algo rung is an H2 tagged with its step + pattern,
    # matching the site's section chrome.
    out: list[str] = []
    if fm.get("kind") == "intro":
        out.append(f"## {INTRO_HEADING}\n")
    else:
        step = fm.get("step", "")
        pattern = fm.get("pattern", "")
        tag = f" ({pattern})" if pattern else ""
        prefix = f"{step}. " if step else ""
        out.append(f"## {prefix}{fm['title']}{tag}\n")

    # Walk the body line by line, swapping widget markers for rendered markdown
    # and passing prose through (HTML stripped). Blank runs collapse naturally.
    for line in body.splitlines():
        stripped = line.strip()
        if stripped.startswith(":::figure"):
            fig = stripped[len(":::figure") :].strip()
            out.append("\n" + render_figure(fig, titles))
        elif stripped == ":::tradeoff":
            out.append("\n" + render_tradeoff(fm))
        elif stripped == ":::recap":
            out.append("\n" + render_recap(fm))
        else:
            out.append(strip_html(line))
    return "\n".join(out).strip() + "\n"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT, help="output markdown path")
    args = ap.parse_args()

    titles = scenario_titles()
    sections = [render_section(name, titles) for name in FILES]
    # References section: footnote definitions, rendered as `[^n]: [url](url)`.
    footnotes = "\n".join(f"[^{i}]: [{url}]({url})" for i, url in enumerate(FOOTNOTES, 1))
    doc = (
        "<!-- Generated by scripts/bloggen.py from web/content/*.md — do not edit by hand. -->\n\n"
        + "\n\n".join(sections)
        + "\n\n## References\n\n"
        + footnotes
        + "\n"
    )
    # Collapse the runs of blank lines the marker swaps leave behind.
    doc = re.sub(r"\n{3,}", "\n\n", doc)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(doc)
    print(f"wrote {args.out} ({len(doc)} bytes, {len(sections)} sections)")


if __name__ == "__main__":
    main()
