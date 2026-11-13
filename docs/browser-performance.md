# Browser performance: first optimization pass

This is an internal optimization of the existing revision-2 native PNG viewer,
not a new transport or a claim of video-rate rendering. Page input, ownership,
generation, exact sequence ACKs, PNG limits and real Chrome-owned DevTools retain
their existing behavior.

## Changes

- Each visible surface reuses the last fully validated PNG's `RenderImage` when
  the encoded bytes and dimensions match. Frame metadata is still checked on
  every reply; the worker still validates generation/sequence and consumes every
  successful frame, including a pixel-cache hit.
- A matching image avoids PNG/base64 decoding, RGBA-to-BGRA conversion, image
  allocation and creation of another GPUI image identity. Snapshot publication
  compares image identity, so unchanged pixels do not request another repaint.
- Unchanged tab metadata advances its ordering watermark without repainting.
  Admission and DevTools open/close publish their own semantic changes, even
  with no tabs or a completely static page.
- There is only one cache per page/DevTools surface, not a history per tab. It
  owns encoded bytes and a **weak** image reference: clearing the displayed
  snapshot does not leave decoded pixels owned by the cache. Inactive caches
  are cleared on the next frame-loop pass; an in-flight capture may finish
  before that pass. Revocation/disconnect drops the worker and its caches.
- Missing displayed pixels are restored even if a new frame matches a cached
  image. A good frame still clears a previous frame error without clearing an
  unrelated input/command diagnosis.
- The parent `herdr-forks` launcher now defaults to
  `CARGO_PROFILE_DEV_OPT_LEVEL=1` for its daily-use app/core builds. It keeps the
  cached development profile and ordinary development checks, rather than doing
  a full release build. Set `CARGO_PROFILE_DEV_OPT_LEVEL=0` explicitly for faster
  unoptimized development builds. Standalone Cargo commands are unchanged.

## Measurement

CPU-only microbenchmark on the original Ubuntu host, Rust 1.95.0, dependency
optimization as declared by this repository. A deterministic 1280x720 image
produces a 1,082,315-byte PNG. Both paths process 30 identical frames with changing
transport timestamps and retain a displayed image, as the viewer does. Cached
timing includes the first full decode. There is no timing threshold in CI.

At the launcher's optimized development profile:

| Path | 30 frames |
| --- | ---: |
| Full decode for every frame | 441.4 ms |
| Last-frame cache, including first decode | 27.7 ms |

That is approximately **15.9x less decoding time for this static workload**.
An earlier unoptimized-profile measurement took 2.26 seconds for the uncached
path; selecting opt-level 1 also helps the non-cached path. These are not total
browser CPU, GPU, network, latency or end-to-end FPS measurements. Changing
content will miss the cache, and normal pages vary substantially.

Reproduce from the app checkout after installing developer prerequisites:

```bash
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_OPT_LEVEL=1 \
  cargo test --locked --bins \
  benchmark_static_frame_decode -- --ignored --nocapture
```

## Validation and remaining work

- App `just check`: **440 Rust tests and 76 Bun tests passed**, plus native
  headless pixel checks. Two pre-existing tests remain ignored; the new manual
  benchmark is ignored by the normal suite and was executed separately.
- Regression coverage: changed/unchanged pixels, malformed metadata on a cache
  hit, failed decode preserving a good cache, no per-tab history, weak image
  lifetime/cache eviction, hidden/closed surfaces, restoration, frame-error
  recovery, metadata ordering, empty admission, static-page dock open/close,
  existing rapid A/B/A follow and revoked-generation rules.
- Independent read-only review found and then verified fixes for semantic
  publication and decoded-image retention; it did not reproduce timings.
- Parent launcher: **23 Python tests**, shell syntax/ShellCheck and a real
  `./herdr build` passed. No production service or active browser was restarted.
- An exploratory repository-wide `cargo fmt --all -- --check` reports existing
  formatting in untouched files such as `build.rs`; no mass reformat was made.
- No new real-desktop input, native browser acceptance, Arch installation or
  compositor/GPU performance measurement is claimed for this pass.

Chrome still captures and PNG-encodes frames at the existing bounded polling
cadence, and full frame bytes still cross the wire. This pass does **not** reduce
Chrome capture/compression or transport bandwidth. Those costs, interaction
latency, video/animation and compositor behavior need separate profiling before
changing capture scheduling, quality or protocol semantics.
