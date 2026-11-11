# Workspace browser feasibility gate (not a shipped feature)

Stage A is **not passed**. There is no Browser outer tab, native streamed page,
DevTools surface, or implemented browser client protocol in this app. Do not use
this branch for logged-in browsing or infer support from the test fixture.

The shared handoff and core-owned proposal are in
`/home/firesquid/documents/herdr-workspace-browser`. The proposal is not a running
API. No browser capability should be advertised until implementation and real
native gate verification exist.

## Isolated app sandbox

The existing sandbox now accepts an explicit scratch root and test-core binary:

```sh
export HERDR_APP_SANDBOX=workspace-browser-app
export HERDR_APP_SANDBOX_ROOT=/home/firesquid/.hermes/cache/scratch/workspace-browser-app/sandbox
export HERDR_APP_SANDBOX_CORE=/absolute/path/to/sibling/feature/target/debug/hc
scripts/sandbox up
scripts/sandbox status
scripts/sandbox shot /absolute/path/outside/sandbox/app.png
scripts/sandbox down
```

Only run `up` after the core owner supplies an actual feature binary and agrees
to combined resources. Never substitute the production core. The app still needs
Cargo, its native dependencies, Weston and Xwayland; screenshot/input commands
also need the tools named in `scripts/sandbox`. There is no backend coverage
merely because the script passes isolation tests.

The root must be new or owned by this sandbox's marker (UID and sandbox name).
An old unmarked sandbox is not adopted/deleted automatically: choose a fresh
root instead. Names accept letters, digits, underscore and hyphen. Root paths
must be canonical absolute paths, not `/`, HOME or symlinks. XDG config, cache,
data, state and runtime, plus TMPDIR and app home, stay beneath the root.
`HC_SESSION` is `herdr-app-<name>`; choose a distinct name for simultaneous runs.
The explicit test core is linked into the scratch PATH, not installed. Cleanup
checks process environment ownership before signaling the window/compositor;
it stops only this HC_SESSION. `down` removes the owned root, so keep evidence
outside it. Never manually supply real display coordinates to the input helper.

Safety tests (no GUI, core or desktop input):

```sh
PYTHONDONTWRITEBYTECODE=1 TMPDIR=/home/firesquid/.hermes/cache/scratch/workspace-browser-app \
  python3 -m unittest discover -s scripts -p 'test_sandbox.py' -v
```

## Synthetic HTTP fixture

```sh
PYTHONDONTWRITEBYTECODE=1 python3 tests/workspace-browser/server.py
```

This foreground process prints its loopback-only ephemeral origin; Ctrl-C closes
it. Open that origin only through the eventual workspace browser, on the same
host as this fixture. For an isolated remote core, run the fixture there. It has
no Herdr bridge and no browser automation endpoints. Uploads are at most 1 MiB,
hashed in memory and never written to disk. All data must be synthetic. The
fixture is test-only and should not be exposed on a network interface.

Fixture self-tests use actual loopback HTTP and close/join their own servers:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s tests/workspace-browser -p 'test_fixture.py' -v
```

These prove fixture behavior, **not** native rendering/input, Herdr uploads,
downloads, authentication, clipboard, sandboxing, or DevTools.

## Gate exercise once unblocked

- Type a unique unsent draft; note the displayed memory UUID. Hide the outer tab,
  disconnect, and reconnect. Both must remain unchanged; a reload deliberately
  changes the UUID. Record core target identity as well.
- Inspect `#inspect-me` and its `data-fixture` attribute in the real embedded
  bottom-docked remote DevTools. Evaluate `window.fixtureMemory` in its console.
  Make the HTTP request and inspect `/request` in Network; `/observed` records
  actual HTTP hits, not screenshot-inferred success. Run the debugger button,
  step `value += 2`, and verify 42 after resume.
- Use popup button and target=_blank link; verify opener message, not just URL.
  Set the synthetic cookie, verify inner-tab sharing and a second workspace's
  isolation. Never store the unsent draft in localStorage to fake survival.
- Select/copy/paste text, compose non-ASCII text, use the native select, drag
  text, scroll to `#bottom`, resize and compare CSS viewport/DPR at 1× and HiDPI.
  Inventory actual reserved bindings versus forwarded keys and focus escape.
- Choose a synthetic upload explicitly; compare its SHA-256 and byte count
  with `/observed`. Download the fixed fixture file to the host, then explicitly
  copy to desktop and compare bytes. Fixture self-tests bypass these UI paths
  and cannot count as transfer acceptance.
- Handle the JavaScript prompt; verify geolocation denial without granting a
  real location. Test two actual native clients for blocked/takeover/revocation,
  stale input/DevTools, lease expiry and browser survival.
- Test core restart separately from browser restart, profile persistence,
  resource cleanup, machine routing across independent isolated endpoints,
  terminal/layout regression, old-core support errors and malformed streams.

No performance threshold is claimed. Measure frame sizes, cadence, decode/upload
cost, input latency and idle resource use only against a real end-to-end run.
Record PASS/FAIL/NOT TESTED separately for every check; never substitute the
fixture page or protocol proposal for the required native gate.
