# Native renderer ACK / clipboard transport counterexample

**Historical direct-renderer experiment: `runtime_missing_contract`.**
That receipt is not the verdict on the later integrated production protocol.
The original experiment used no GPUI/OS clipboard, real desktop, core,
credentials, Cargo, downloads, host changes, grants or production edits. This directory contains source/docs/scripts/summaries only. The original
five runtime evidence directories were moved without deletion to
`$HOME/.cache/herdr-browser-prototype/clipboard-transport-proof/` (normal-user
HOME); all 40 retained files were checked byte-identical across the move.

## Integrated production-module handoff

`../../src/browser/clipboard_shuttle.rs` is integrated with the runtime, renderer
hooks and native pane. Its 15 pure tests passed as part of 476 native unit passes;
parent private Wayland run 14 passed actual OS/app acceptance, including the
reproduced queued-Paste pressure case. See `../../docs/browser-context-menus.md`.
The original standalone receipts below remain distinct historical experiments;
they are not production or OS clipboard acceptance.

The revised operational contract accepts bounded shared-clipboard observation;
it does not require crypto writer attribution or a formal engine commit callback.
The original ACK/cross-pipe finding below remains true but is **not by itself a
custom-runtime necessity argument** for this revised module.

### Required parent integration

1. Declare the module under the existing browser/Linux gate. After lazy CEF
   initialization, construct one UI-thread `Coordinator::new(now)`; construction
   is process-global/once-only. Do not initialize CEF for remote/no-pane sessions.
2. Put an `Arc<RendererHooks>` on the application's RenderProcessHandler in both
   main/subprocess construction. Forward native `on_browser_created` **with
   extra_info**, `on_browser_destroyed`, and process messages. Only native
   browser-created private marker metadata registers the helper. An ordinary
   `about:blank` browser is NOT trusted by URL.
3. Outer runtime pumps CEF, then calls actor `tick(now)`; actor callbacks use only
   Arc/Mutex mailboxes and never borrow runtime RefCell/UI state. Context
   readiness, **asynchronous browser creation**, load and init ACK all precede
   `Status::Idle`. Never pump/reenter CEF from the actor.
4. Route ALL app clipboard commands/shortcuts through one global coordinator.
   Parent gates availability by current native CanCopy/CanCut/CanPaste edit flags,
   captures `OwnerStamp { owner, document_generation }` and native Browser, then
   calls `begin_copy_cut` or `begin_paste`. Generations/invalidation must cover
   the focused **subframe's** document changes too, not only main-frame loads.
   Paste accepts a fresh parent GPUI
   snapshot: `None` is unavailable; `Some("")` is valid empty plaintext.
5. On `Status::ReadyForSource`, parent revalidates CURRENT app/page/tab/document
   ownership/focus and edit flags, then explicitly calls
   `dispatch_source(id, current_stamp, current_browser, now)`. The actor compares
   its captured native browser and the newly fetched focused frame immediately
   before calling **browser-native** Frame.Copy/Cut/Paste. `tick` NEVER dispatches
   a source action. Paste then sends an adjacent command-free Unidentified keyup
   through internal CEF DevTools and holds global Busy until its correlated
   processing ACK. `PasteDispatched` still does not promise insertion/focus-freeze
   success. The extra keyup is observable. Source Cut is never automatically retried.
6. Drain `take_completion(id)` and revalidate CURRENT ownership, including the
   completion's captured `source: NativeFrameIdentity`, immediately before
   any GPUI export. `CopiedText("")` must write empty text. `NoUpdateObserved` and
   failures must NOT write the OS clipboard. No source DOM is inspected. The
   actor itself accesses no OS/GPUI clipboard.
7. On owner/tab/focus/navigation/close/address-clipboard invalidation call
   `cancel(id)`. This suppresses export. If no work is outstanding it aborts
   immediately; otherwise `AbandonedDraining` stays globally Busy while tick
   drains existing helper/source work. Healthy result/no-update then drops the
   completion and captured source lease, returning Idle **without poison**,
   even if the pane has dropped its ID. Keep ticking independently of PaneView.
   New intents may retry/queue only with bounded lifetime and current revalidation.
8. Ordinary pages independently need both legacy clipboard BrowserSettings
   DISABLED, CLIPBOARD_READ_WRITE BLOCK and permission prompt DENY. The helper
   sets those itself, uses a dedicated non-global ephemeral nonsharing context,
   rejects further navigation/subframes/network/popups/downloads/permissions and
   installs no privileged page API. Ordinary gesture writers remain legitimate.
9. Shutdown: call `shutdown()`, keep pumping/ticking, and drain ordinary pages
   **plus `outstanding_helper_count()`** (live **and pending creates**). The helper
   closes itself even if async creation arrives after timeout/shutdown/drop.
   `is_drained()` / `closing_count()` are also exposed. Only after counts reach
   zero drop actor/CEF references and call CEF shutdown. Do not exclude a pending
   helper create merely because the actor has no Browser handle yet.

### Implemented outcome policy / review targets

- Source Copy/Cut first gets a unique helper sentinel and verified native paste
  readback. Paste first gets exact typed input and verified readback. Helper
  native Copy selects a **nonempty fixed dummy**, and its trusted custom handler
  sets `text/plain` + preventDefault. Chromium normalizes empty text to no-format
  clears; typed `has_formats` distinguishes these from nontext formats, and healthy
  final clears project to empty OS text. Early no-format reads keep polling.
  Parent extracts actual String entries rather than GPUI's empty/file-path coercion.
- Text limit: 64 KiB UTF-8; JS bounds before return IPC and Rust bounds again.
  Values are typed V8/process-message arguments, never interpolation. Pending
  reply authorization includes exact helper browser/main-frame/private marker,
  transaction, nonce, operation and deadline. Stale replies are ignored.
- Policy constants: setup 10 s, seeding 5 s, individual IPC/native helper event
  1 s, source observation window 500 ms, native input fence 3 s. No sleeps or
  claimed Copy/Cut write-commit barrier; native input ordering is conditional on
  same-widget normal delivery, not every possible ignored/suppressed ACK.
  A healthy final sentinel read ends `NoUpdateObserved` and permits the next
  **fresh verified seed**. Healthy no-plaintext/oversize results fail without
  export. Genuine helper IPC/event/seed/protocol failure poisons until restart
  and requests helper close. No reset/recreate recovery API yet.
- Abandonment by itself NEVER poisons. Outstanding work drains boundedly; if
  that drain genuinely fails, the transport failure—not tab switching—poisons.
  Paste data is not immediately scrubbed; shared clipboard persistence and
  independently authorized/late writes are ordinary races, not crypto failures.
- Parent run 14 covers empty values, no-selection/cancellation reuse, actual OS
  round trips, bounds/Undo, tab/focus invalidation and drain, queued source
  pressure/Busy, stale menu owners and child-frame READ denial. The earlier
  renderer-Paste failure remains **UNDETERMINED** and is not the path implemented
  here. Arbitrary renderer suppression and universal website compatibility are
  not certified by these fixed owned fixtures.

## Acknowledged native input probe (separate CEF-only evidence)

`cdp_paste_probe.rs` and `run-cdp-paste.py` add a cache-only adapter of the
unchanged legacy source. It checks 12 cases (native Paste+command-free keyup,
key-command Paste and Ctrl+V; each normal/canceled/empty/preblocked-renderer),
three keyboard-cancellation cases, six pre-consent read denials, a control that
reproduces the old dispatch-return race, and clean owned cleanup. The wrapper
fails on missing/mismatched cases. Latest validated receipt is cache-relative
`cdp-evidence-1794829357189698652/summary.json`.

Native Paste+keyup preserves Paste when a generic keydown handler blocks both
keyboard-Paste candidates. Same-widget normal input queue ordering has pinned
source support, unlike CEF process-message ACK. Method success does not expose
ACK source/state; no unconditional ignored-input guarantee is claimed. This
probe uses native internal DevTools API, not an HTTP debugger endpoint. It makes
**no OS clipboard/app acceptance claim**; parent run 14 provides that separately.

## Separate stock-CEF legacy READ gate receipt (11/11 passed)

New run: cache-relative `legacy-evidence-1794810611108875168/`; executable
`legacy-read-proof`, exit **0**. `legacy-summary.json` is a **separate** result,
verified by `verify-legacy.py`; it does not revise the original 80/20 result.
Only this coupled small test ran; the 100-operation experiment was not rerun.

The pinned `cef-156.4.0+156.0.26` bindings expose the exact BrowserSettings fields
`javascript_access_clipboard` and `javascript_dom_paste` (Linux x86_64 generated
bindings lines 750–751). `legacy_probe.rs` explicitly sets both to
`State::DISABLED` on A, B and the shuttle, retains private ephemeral contexts,
`CLIPBOARD_READ_WRITE=BLOCK`, permission-prompt DENY, and the original typed
browser/frame/nonce IPC and sandbox/helper classifier. No permission prompt
callback was needed in this run.

After browser-native shuttle copy and exact shuttle readback of a fixed owned
Unicode/whitespace fixture, the following ordinary website APIs were exercised
**before any native Paste consent on A/B**:

| Condition | `document.execCommand('paste')` | `readText()` | `read()` |
|---|---|---|---|
| A focused, nonactivated | false, unchanged, no event | DENIED | DENIED |
| A focused, actual CEF-click activated | false, unchanged, no event | DENIED | DENIED |
| Independent B background/nonactivated | false, unchanged, no event | DENIED | DENIED |

The actual owned-page CEF mouse click established `userActivation.isActive=true`;
a subsequent ordinary selection operation reselected the editable field. Focus,
activation and selection were recorded at execution. `execCommand` is called by
ordinary fixed website JS through the same V8 evaluator, **not** a native Paste
helper or privileged page binding. There are no website API replacements,
grants, CDP policy overrides, or network changes. API promise success would be
recorded ALLOWED even if later result inspection failed.

All nine negative cases kept `UNCHANGED`, reported no seed observation in the
editable field/DOM/event data, and produced no native paste event. Runtime logs
contain booleans/lengths only, never the raw fixed seed. These checks cover the
owned fixture's sinks and tested APIs, not every possible website read path.

Only afterward, **browser-process** `Frame.Paste` into focused A delivered exact
seed data in trusted native events with the same DISABLED settings:

- Default: nonempty exact clipboardData (36 UTF-16 units), exact destination,
  trusted `paste`, `beforeinput`, `input` (`insertFromPaste`).
- Canceled: nonempty exact clipboardData, trusted prevented `paste`, destination
  still `UNCHANGED`, no input edit. Reading consented clipboardData is normal.

All three browsers closed, CEF shutdown completed, no remaining owned process
was found; helper-classifier tests passed 4/4. **Receipt scope: stock-CEF
semantics and the tested unattended READ gate, not production acceptance,
OS/GPUI clipboard proof, or proof that a custom runtime is necessary.** The B
native-command interleavings below remain mechanism counterexamples, not a
serialized-coordinator bypass. No crypto provenance or immediate scrub contract
is asserted. The earlier renderer-side Paste empty-data cause remains
**UNDETERMINED**; this browser-side test neither fixes nor reclassifies it.

```sh
bash /home/firesquid/worktrees/herdr-forks/browser-clipboard-native-app/prototypes/clipboard-transport-proof/run-legacy.sh
python3 /home/firesquid/worktrees/herdr-forks/browser-clipboard-native-app/prototypes/clipboard-transport-proof/verify-legacy.py
```

`run-legacy.sh` uses the same cached rustc/CEF/serde artifacts and private harness
as `run.sh`, compiling `legacy_probe.rs` to a new cache-only `legacy-evidence-*`
directory with a 45-second native run limit. No Cargo or real desktop access.

## Original ACK experiment: why ACK is not a barrier

The independent source review supplied with this task found that the renderer
ACK uses the CEF BrowserFrame remote, while clipboard Write/Commit uses a
separate `HeapMojoRemote<ClipboardHost>` / newly bound pipe. FIFO on one pipe
**does not order the other pipe**. `CommitWrite` may also be asynchronously
policy-deferred. Returning from the public renderer Frame API, then ACKing, is
not a browser-side clipboard commit acknowledgement. Repeating a successful
schedule cannot establish that missing contract. This experiment does not
independently re-review or attach source-line citations to that supplied finding.

Even with a commit barrier, a shared clipboard plus sentinel inequality does
not attribute a value to the requesting owner. A different authorized writer
can replace the sentinel after A cancels/does nothing. No production bridge is
enabled here; the logs model the rejected export decision only.

## Final retained run

All `evidence-*` paths below are relative to the cache evidence root
`/home/firesquid/.cache/herdr-browser-prototype/clipboard-transport-proof/`,
**not this repository**. Historical log/result contents remain unchanged and may
mention their original pre-move executable/artifact paths; do not treat those
historical paths as current locations.

`evidence-1794809931978689706/run/{app.log,result.json}`; binary:
`evidence-1794809931978689706/clipboard-transport-proof`.
Exit **1**, intentionally retaining the missing-contract failures.
`summary.json` is verified by `verify.py`.

- **10 complete semantic repeats, 100 source operations:** 80 copy/cut cases
  passed; **20 renderer-side paste cases failed exact-data delivery** (trusted
  paste events carried empty text). Default paste emptied the destination;
  canceled paste preserved it. **Cause: UNDETERMINED.** This observation does
  not imply a policy fix is needed. No permission/policy workaround was applied.
- Copy/cut: default exact `" \n🦀 café 中文\nlast\t "`, custom event-provided
  `"CUSTOM 🦀\n exact"`, canceled, and zero-selection cases, 10 each.
  All source events observed were trusted. Native cut included
  `beforeinput/input` with `deleteByCut`; default paste included those events
  with `insertFromPaste`. Custom copy/cut used `setData` + `preventDefault`.
- All **40 canceled/zero-selection copy/cut** cases retained their unique
  transaction sentinel and modeled **no export**. This is empirical evidence,
  not a proof that no later clipboard write could occur.
- **4 deterministic mechanism-level wrong-owner counterexamples:** copy/cut ×
  cancel/no-selection. **Not a demonstrated app-coordinator bypass:** a fully
  globally serialized app coordinator could exclude B's explicit native Copy
  command. These cases show the sentinel-inequality mechanism's lack of writer
  attribution under the controlled interleaving, not a proven production race.
  Seed unique sentinel → renderer A native action + return ACK → proof-only
  snapshot and read establish unchanged sentinel → unrelated B explicit native
  Copy → shuttle extraction returns exact unique B text, not sentinel.
  `old_algorithm_would_export_wrong_workspace=true` in all four cases.
  The extra pre-B read deliberately establishes the no-write precondition;
  it is not offered as an application completion protocol. B's copy was a
  separately authorized native command on the owned fixture, not a website grant.
- Focused nonactivated A website read: **DENIED**. Independent background B
  website read: **DENIED** (focus and activation logged). All three private
  ephemeral request contexts configure `CLIPBOARD_READ_WRITE=BLOCK`; permission
  prompts are handled with DENY. No prompt callback invocation was needed in
  this run. No claim of strict denial inside explicit native Paste consent.
- Owned CEF mouse click activated B; normal website `writeText` was **ALLOWED**,
  with exact shuttle readback. This is normal activated-write behavior, **not**
  an isolation blocker. This separate website-write result does **not** prove
  that an activated website write races the canceled A transaction. No combined
  A-canceled-copy-handler/async-website-writer case was run. No OS/compositor
  input was injected.
- All three `on_before_close` callbacks and CEF shutdown completed. A separate
  deliberate assertion run (`evidence-1794809930522515433`) also closed all three
  browsers and shut down, exit 1. `verify.py` found no surviving executables or
  command lines associated with these experiment runs at its post-run snapshot.

### Protocol actually exercised

Browser→renderer messages contain a typed browser ID and typed frame ID, nonce,
operation, text and mode strings. Renderer checks callback browser/frame IDs,
valid main frame, bounded nonce/text and allowlisted operations/modes. Native
copy/cut/paste executes **directly on the public renderer-side Frame API**;
only after it returns does the renderer send ACK. Browser accepts replies only
from the exact pending nonce/browser/frame. No privileged page binding exists.
Fixed fixture inspection uses V8 string arguments, not page-data interpolation;
the owned DOM log is **proof only**, never production authority.

Hidden app-owned `about:blank` shuttle uses browser-side native Frame copy/paste;
A/B use separately scoped owned loopback HTTP URLs. All context pairs are checked
not to share; no persistent cache path is set on those contexts. Extraction is
**one shuttle paste after source ACK**, not an expected-value retry. Shuttle is
prepared before the source command. Only fixture **seeding** retries until the
seed is observed. The pump's 3 ms yield is scheduling, not completion evidence.
There is no 150 ms cancellation settling window or deliberate heavy task queue.

### Retained earlier diagnostics (not hidden)

- `evidence-1794809780576016566`: initial all-renderer shuttle seeding failed;
  no semantic repeat completed. Switched the app-owned shuttle to the requested
  browser-side native operation; did not alter source renderer operations.
- `evidence-1794809810157985811`: 8 copy/cut cases passed, then first renderer
  paste failed; closed cleanly. Updated diagnostics to retain paste failures
  and continue to the competing-writer experiment, not to mask them.
- `evidence-1794809843627143165`: earlier complete 10-repeat run, same 80/20
  copy-cut/paste result and 4 attribution counterexamples, clean shutdown.
- Final rerun followed addition of live-browser shutdown guarding and assertion
  injection. Thus **two full 10-repeat runs**, plus the above incomplete warmups;
  no claim that 100 repetitions of every scenario were performed.

## Reproduce (normal user, cached artifacts only)

```sh
bash /home/firesquid/worktrees/herdr-forks/browser-clipboard-native-app/prototypes/clipboard-transport-proof/run.sh
# Expected exit 1: runtime_missing_contract, not infrastructure success.
PROOF_INJECT_ASSERT=1 bash /home/firesquid/worktrees/herdr-forks/browser-clipboard-native-app/prototypes/clipboard-transport-proof/run.sh
python3 /home/firesquid/worktrees/herdr-forks/browser-clipboard-native-app/prototypes/clipboard-transport-proof/verify.py
```

`run.sh` resolves the original normal-user HOME **before** `env.sh` and before
the private harness, creates future binaries/logs only in
`$HOME/.cache/herdr-browser-prototype/clipboard-transport-proof/evidence-*`, and
runs exactly:

```sh
rustc --edition=2024 "$HERE/probe.rs" \
  -L "dependency=$P/target-app-mvp/debug/deps" \
  -L "native=$P/runtime-156.0.26" \
  --extern "cef=$P/target-app-mvp/debug/deps/libcef-ad1108621cfb5a08.rlib" \
  --extern "serde_json=$P/target-app-mvp/debug/deps/libserde_json-9f3941d56b23a919.rlib" \
  -C debuginfo=0 -o "$RUN/clipboard-transport-proof"
```

Here `P=$ORIGINAL_HOME/.cache/herdr-browser-prototype` and
`RUN=$P/clipboard-transport-proof/evidence-<timestamp>`. The script resolves
those absolute paths before the harness replaces HOME. `verify.py` likewise
uses the normal-user HOME and reads the relocated cached logs; it runs no native
experiment. The earlier packaging-only pass changed no Rust source behavior
and reran no native test. The later separate legacy run is documented above.
Runtime and `LD_LIBRARY_PATH` are `execution-stage-156.0.26`.
The existing private `run-headless.py` harness owns process-group timeout/cleanup,
new HOME/XDG/Weston, DISPLAY unset, no XWayland, 100 s application bound.
No seat module was needed. CEF itself explicitly uses Ozone **headless**;
Weston isolation does not turn this into a Wayland/OS clipboard test.

`arguments.rs` is byte-identical to the old probe's full-argv sandbox classifier;
its **4 tests passed**. `Settings.no_sandbox=0` and helper dispatch/sandbox
assertions are preserved. New shell script passes `bash -n` and ShellCheck.
Prior source and evidence were read only. Production/menu files were not edited.

Original source/docs/scripts: `probe.rs`, `scenarios.rs`, `arguments.rs`, `run.sh`,
`verify.py`, `summary.json`, this `README.md`, and `.gitignore`.
Separate legacy files: `legacy_probe.rs`, `legacy_scenarios.rs`,
`legacy-fixture.js`, `run-legacy.sh`, `verify-legacy.py`, `legacy-summary.json`.
No binaries or
runtime journals remain in this prototype directory. `.gitignore` excludes
accidental local build/runtime scratch. Each cached `evidence-*` retains its
binaries, build/test logs and harness artifacts; scratch homes are retained by
the existing harness under the `workspace` path in each `result.json`.
