# Core-owned browser follow-up (incomplete)

This branch follows `integrate/upstream-browser` (`854d197`), not the historical
CEF clipboard branch. It preserves the integrated core-owned Chromium runtime;
app PR5's old CEF receipts do not certify this port.

Implemented: replaceable trusted chrome around a native BrowserPage component;
scoped commands/keys and explicit global precedence; native page/address menus;
serialized UI-thread plaintext OS clipboard writes and fresh bounded Paste reads;
workspace-shared viewer connections and pane-owned core groups; inert saved-layout
migration; confirmed URL/order/selection projection; warm retained live targets.

No website/mod privilege bridge, selected-source-text clipboard mirror, synthetic
clipboard events, clipboard-backed IME or broad website permission grants. Plaintext
is at most 65,536 UTF-8 bytes; empty String entries survive and file paths are not
coerced. Exact captured owner identities are never retargeted. Native input ACK is
not ClipboardHost write-commit proof. Cut is never automatically retried or undone.
Remote creation/attachment refuses desktop-local fallback.

## Checked, not inferred

- App binary unit suite: **558 passed, 2 ignored**. Script tests: **161 passed**.
  Full `just check` also passed with isolated HOME/XDG paths and serial Rust tests;
  native-desktop-only ignored tests remain distinct from the owned native fixtures.
- Private native-Wayland slice on final standard-build app/core binaries: native OS Copy/Paste, Cut once,
  trusted custom page menu, native address menu/expiry/Escape cases, all seven
  physical aliases, ambient read denial and observed stop/cleanup.
- Separate private two-pane native slice: different groups/targets and independent
  OS witnesses, one engine instance/PID, lazy saved entries, retained live
  form/marker/history across selection and same-instance reattach, resumed native
  input, rejection of unfocused-pane Copy, stop observed in both attached panes,
  no tracked survivors. This is not a full lifecycle/performance matrix.

Receipts, exact binaries/hashes and failed predecessors live in the parent fork's
`docs/browser-pane-groups-progress.md` and `docs/browser-completion-native-progress.md`.
Core headless probes are separate from native-Wayland/GPUI acceptance. Default
library/stub tests are not real desktop acceptance.

## Not complete

#4 still has cancellable-keydown menu semantics, non-atomic focus/dispatch/Cut
checks, no clipboard host commit fence, two retained required positive core native
failures (diverted focus and OOPIF), and unsupported custom-shadow editing. Safe
rejection prevents wrong export; it does not satisfy positive functionality.

#2 still lacks cold restart/persistence acceptance, loading/favicon UI and actual
hidden-page throttling. Broader terminal isolation, popup/reopen/move/drag UI,
TLS/file/profile/machine/IME/accessibility/performance/DevTools/auth matrices and
containment/worker/remote work remain open. Core groups count empty records toward
16; visibility is bookkeeping, not throttling. A URL reopen is a new page, not
restored form/history/JavaScript. No issues should auto-close from this checkpoint.
