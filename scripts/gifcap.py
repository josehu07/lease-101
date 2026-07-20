#!/usr/bin/env python3
"""Capture each walkthrough scenario canvas to a looping GIF.

Drives the *real* built web app through a headless browser, so a captured frame
is pixel-identical to what a reader sees on the site. For each scenario it loads
the app's `/capture/:name` route (a bare, frame-stepped `.pg-stage` with no page
chrome — see `web/src/scenarios.rs::Capture`), steps every recorded frame via the
`window.__setFrame(n)` bridge the route installs, screenshots the stage, and
encodes the frames into a GIF that loops forever with a short static hold at both
ends.

Prereqs (installed via `uv sync` + `uv run playwright install chromium`):
  - a built dist at `target/dx/lease_web/debug/web/public` (run `dx build` first;
    or point `--dist` at a release build)
  - Playwright's Chromium.
  - `gifsicle` on PATH for post-optimization (optional but recommended, e.g.
    `brew install gifsicle`); without it the GIFs are still correct, just larger.

Usage:
  uv run scripts/gifcap.py                 # all scenarios -> docs/gifs/
  uv run scripts/gifcap.py roster-leases   # just one
  uv run scripts/gifcap.py --scale 2       # 2x device pixels (crisper, bigger)
"""

from __future__ import annotations

import argparse
import functools
import http.server
import io
import shutil
import socket
import socketserver
import subprocess
import threading
from pathlib import Path

from PIL import Image
from playwright.sync_api import sync_playwright

# Repo root is one level up from this scripts/ dir.
ROOT = Path(__file__).resolve().parent.parent
DEFAULT_DIST = ROOT / "target/dx/lease_web/debug/web/public"
DEFAULT_OUT = ROOT / "docs/gifs"
# GIF filenames are `<GIF_PREFIX><scenario>.gif`, prefixed to namespace them in the
# blog site's shared asset dir. Kept in sync with `bloggen.py`'s GIF references.
GIF_PREFIX = "lease-101-"

# The wired scenarios, matching `web/src/scenarios.rs::lookup` and the `:::figure`
# markers in `web/content/*.md`, in walkthrough reading order.
SCENARIOS = [
    "one-to-one-success",
    "one-to-one-guard-reply-lost",
    "one-to-one-revoked",
    "one-to-one-renew-replies-lost",
    "lease-manager-handover",
    "leader-leases",
    "quorum-leases",
    "quorum-leases-write-disruption",
    "roster-leases",
]

# Browser playback pace: the live canvas advances one recorded frame every
# `RENDER_MS` (see `web/src/sim_view.rs`). Reproducing that per-frame duration in
# the GIF keeps the wall-clock speed identical to the site.
BROWSER_FRAME_MS = 7

# Post-optimization with gifsicle (if on PATH): `-O3` frame-diff optimization plus
# a palette+lossy pass. The canvas uses few flat colors, so 64 colors and a strong
# lossy level shrink the busy scenes substantially (~35% off the dense ones) while
# staying visually indistinguishable at display size. Skipped (with a warning) if
# gifsicle isn't installed — the un-optimized GIF is still correct.
GIFSICLE_ARGS = ["-O3", "--colors", "64", "--lossy=200"]


class SpaHandler(http.server.SimpleHTTPRequestHandler):
    """Static file server with a single-page-app fallback.

    Serves files from the dist dir; any path that isn't a real file (e.g. the
    client-side route `/capture/roster-leases`) falls back to `index.html`, just
    like the GitHub Pages 404.html deploy fallback. Assets are referenced by
    absolute path, so they resolve regardless of the route depth.
    """

    def send_head(self):  # type: ignore[override]
        path = self.translate_path(self.path)
        if not Path(path).is_file():
            self.path = "/index.html"
        return super().send_head()

    def log_message(self, *args: object) -> None:  # silence per-request logging
        pass


def serve(dist: Path) -> tuple[socketserver.TCPServer, int]:
    """Start the SPA server on a free port in a background thread."""
    handler = functools.partial(SpaHandler, directory=str(dist))
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    httpd = socketserver.TCPServer(("127.0.0.1", port), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd, port


def build_palette(frames: list[Image.Image]) -> Image.Image:
    """A single 256-color palette covering every frame's colors.

    Reusing one palette across all frames (rather than a per-frame adaptive one)
    keeps flat regions stable — no palette flicker between frames. Built from a
    vertical montage of an evenly-spaced sample of frames so the whole run's
    colors (e.g. green auras that only appear once leases hold) are represented.
    """
    step = max(1, len(frames) // 30)
    sample = frames[::step] or frames[:1]
    w = sample[0].width
    montage = Image.new("RGB", (w, sum(f.height for f in sample)))
    y = 0
    for f in sample:
        montage.paste(f, (0, y))
        y += f.height
    return montage.quantize(colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)


def capture(page, name: str, base: str, stride: int) -> list[Image.Image]:
    """Load `/capture/:name`, step its frames, return the kept ones as RGB."""
    page.goto(f"{base}/capture/{name}", wait_until="load")
    if page.query_selector("#capture-error"):
        raise SystemExit(f"unknown scenario: {name}")
    # The route sets `__captureReady` once the frame-stepping hook is installed.
    page.wait_for_function("window.__captureReady === true", timeout=30_000)
    count = page.evaluate("window.__frameCount")
    if not count:
        raise SystemExit(f"{name}: no frames generated")

    stage = page.locator(".pg-stage")
    # Layout is constant across frames (fit-height box is fixed), so fix the clip
    # box once from frame 0 — guarantees every screenshot has identical pixels.
    page.evaluate("window.__setFrame(0)")
    page.wait_for_function(
        "document.querySelector('.pg-capture-stage')?.getAttribute('data-frame') === '0'"
    )
    bbox = stage.bounding_box()
    clip = {
        "x": round(bbox["x"]),
        "y": round(bbox["y"]),
        "width": round(bbox["width"]),
        "height": round(bbox["height"]),
    }

    frames: list[Image.Image] = []
    for n in list(range(0, count, stride)) + ([count - 1] if (count - 1) % stride else []):
        page.evaluate(f"window.__setFrame({n})")
        page.wait_for_function(
            "n => document.querySelector('.pg-capture-stage')?.getAttribute('data-frame')"
            " === String(n)",
            arg=n,
        )
        png = page.screenshot(clip=clip)
        frames.append(Image.open(io.BytesIO(png)).convert("RGB"))
    return frames


def encode_gif(frames: list[Image.Image], out: Path, delay_ms: int, hold_ms: int) -> None:
    """Encode frames to a looping GIF with static holds at both ends."""
    palette = build_palette(frames)
    paletted = [f.quantize(palette=palette, dither=Image.Dither.NONE) for f in frames]
    # Per-frame durations: linger on the first and last so the start/end read
    # clearly before the loop restarts.
    durations = [delay_ms] * len(paletted)
    durations[0] = hold_ms
    durations[-1] = hold_ms
    out.parent.mkdir(parents=True, exist_ok=True)
    paletted[0].save(
        out,
        save_all=True,
        append_images=paletted[1:],
        duration=durations,
        loop=0,  # loop forever
        disposal=1,
        optimize=True,
    )


def optimize_gif(path: Path) -> bool:
    """Shrink `path` in place with gifsicle. Returns False (with a warning) if
    gifsicle isn't installed, leaving the un-optimized GIF untouched."""
    if shutil.which("gifsicle") is None:
        return False
    subprocess.run(
        ["gifsicle", *GIFSICLE_ARGS, "--batch", str(path)],
        check=True,
        capture_output=True,
    )
    return True


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("names", nargs="*", help="scenario names (default: all)")
    ap.add_argument("--dist", type=Path, default=DEFAULT_DIST, help="built web dist dir")
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT, help="output GIF dir")
    ap.add_argument(
        "--scale", type=float, default=1.0, help="device pixel ratio (1 = on-page size)"
    )
    ap.add_argument("--width", type=int, default=900, help="viewport width (>= page max-width)")
    ap.add_argument(
        "--min-delay-ms",
        type=int,
        default=20,
        help="target GIF frame delay; sets the frame stride (GIF floor ~20ms)",
    )
    ap.add_argument("--hold-ms", type=int, default=600, help="static hold at each end")
    args = ap.parse_args()

    names = args.names or SCENARIOS
    unknown = [n for n in names if n not in SCENARIOS]
    if unknown:
        raise SystemExit(
            f"unknown scenario(s): {', '.join(unknown)}\nknown: {', '.join(SCENARIOS)}"
        )
    if not (args.dist / "index.html").is_file():
        raise SystemExit(f"no built dist at {args.dist} — run `dx build --platform web` first")

    # Subsample recorded frames so each GIF frame lasts ~min-delay-ms while keeping
    # the browser's wall-clock speed (each kept frame spans `stride` browser frames).
    stride = max(1, round(args.min_delay_ms / BROWSER_FRAME_MS))
    delay_ms = stride * BROWSER_FRAME_MS

    httpd, port = serve(args.dist)
    base = f"http://127.0.0.1:{port}"
    try:
        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            page = browser.new_page(
                viewport={"width": args.width, "height": 900},
                device_scale_factor=args.scale,
            )
            have_gifsicle = shutil.which("gifsicle") is not None
            if not have_gifsicle:
                print("warning: gifsicle not on PATH — GIFs left un-optimized (larger files)")
            for name in names:
                frames = capture(page, name, base, stride)
                out = args.out / f"{GIF_PREFIX}{name}.gif"
                encode_gif(frames, out, delay_ms, args.hold_ms)
                raw_kb = out.stat().st_size / 1024
                if optimize_gif(out):
                    opt_kb = out.stat().st_size / 1024
                    size = f"{opt_kb:.0f} KB (from {raw_kb:.0f})"
                else:
                    size = f"{raw_kb:.0f} KB"
                print(f"{name}: {len(frames)} frames @ {delay_ms}ms -> {out} ({size})")
            browser.close()
    finally:
        httpd.shutdown()


if __name__ == "__main__":
    main()
