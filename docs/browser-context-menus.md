# Browser menus and page-text clipboard (#4)

Private native run 14 passed the listed acceptance matrix, including the queued
Paste race reproduced in runs 11–12. The fix retains the snapshot/global Busy
through an ordered native-input acknowledgment, not merely browser dispatch.
No permission relaxation, source-page binding or custom runtime was introduced.

## Trusted menus and ownership

The trusted `browser-chrome` mod owns page/link/address menus; CEF full and quick
menus are canceled. Website link, selection and edit-state metadata is bounded
plain data, never executable mod code or command names. Native CEF edit flags
govern password/readonly eligibility. Menu rendering never reads the OS clipboard.

Host-issued menu tokens are process-wide unique and one-shot, bound to the actual
native browser/frame/document generation and focus, not merely a pane or durable
tab ID. Tab/navigation/hide/close/focus/chrome-owner changes revoke capabilities.
`sync_tabs` invalidates before swapping native ownership. `open_link_new_tab`
consumes its validated link capability, then calls `browser.tab.new` in the same
pane; `browser.new` and workspace tabs are not substitutes.

Address menus retain the exact GPUI editor focus and UTF-16 selection/value
snapshot; stale text/selection dismisses the menu instead of restoring a range.
Authorized address actions dispatch synchronously through that editor focus.
InputState/textarea expose `selected_range()` (UTF-16) and
`set_context_menu_enabled(false)`. Mouse listeners must be placed on a plain
wrapper: gpui-shell does not wire them on the opaque `Input` itself. The wrapper
fix passed private native address-menu testing without relaxing CEF owner guards.

## Operational clipboard contract

Page-text clipboard is wired through stock CEF, not a required custom runtime.
The minimum operational contract in the parent/root
`docs/native-clipboard-runtime-contract.md` is authoritative; its stronger
optional engine-session proposal is not a requirement for this delivery.

- `browser.copy`, `browser.cut`, `browser.paste`, trusted menu actions and normal
  editing aliases target the focused page or address editor. Page transport is
  plain text only, at most **64 KiB of UTF-8 bytes**. Rich HTML/images and universal
  website Clipboard API interoperability are not promised.
- Source actions use actual browser-native Copy/Cut/Paste and native DOM events,
  preserving custom clipboardData and cancellation. There is no IME insertion,
  selection mirror, source-DOM text extraction or privileged website bridge.
- One app-global actor coordinates page operations through a hidden app-owned CEF
  document with an isolated ephemeral RequestContext: no profile/cookie sharing,
  network/resources, popups or grants. Payloads remain bounded in process memory,
  with no text logging/history/disk persistence.
- Every explicit Paste takes a fresh GPUI text snapshot: absent text is not the
  same as an empty String entry; file paths are not coerced into plaintext.
  Chromium may normalize empty text to a no-format clear; the bridge projects
  that observed clear to empty OS text, not rich-format preservation.
  Exact helper seed readback is verified before native
  source dispatch. Native Paste is followed immediately by a command-free
  Unidentified keyup through CEF's internal DevTools API; the actor stays Busy
  until its correlated native input acknowledgment. No debug port is opened.
  This extra trusted keyup is observable, but no keydown or clipboard DOM event
  is synthesized. The same-widget normal-input queue ordering is documented in
  the parent runtime contract. Method success does not expose ACK source/state
  and is not an unconditional input-suppression guarantee. Detected frame/agent
  loss, error or timeout disables reuse rather than retrying. There is no
  immediate post-Paste scrub, preserving eligible native-Paste reads.
- Copy/Cut verifies a fresh unique seed, dispatches the native source action, then
  boundedly observes actual native plaintext through helper Paste. Only observed
  non-sentinel text, including a valid empty result, may be exported to GPUI/OS
  while ownership remains current. An unchanged seed is **NoUpdateObserved**, not
  proven native cancellation, successful copying or permission to export a seed.
- Healthy NoUpdateObserved performs no OS write and permits a NEW explicit user
  command with a fresh seed. Never automatically retry Cut: deletion may already
  have occurred. Genuine helper/seed/transport failure poisons page transport
  until app restart; another fixed delay is not a completion barrier.
- Captured native browser/frame/document-generation/focus ownership is rechecked
  before source dispatch and OS export. Tab/navigation/hide/close/focus/hot-reload
  changes abandon export and **drain** pending helper work; stale work is never
  retargeted to a replacement page. Abandonment does not undo an already-dispatched
  native action.
- Ordinary unattended website async and legacy reads are denied, including
  activated reads outside native Paste: `javascript_access_clipboard` and
  `javascript_dom_paste` are disabled, CLIPBOARD_READ_WRITE is blocked and prompts
  are denied. Gesture writers and eligible native-Paste-handler reads are normal
  consented behavior. Website `writeText()` currently uses CEF's private buffer;
  direct website-copy-button-to-OS bridging is not promised. The OS clipboard is
  globally shared across normal workspaces; permitted independent writers remain
  normal races, not proof of transaction-private attribution.

Address editing and link-address copying use ordinary GPUI clipboard paths,
separate from page transport. Page command return means **accepted asynchronously**,
not a completed clipboard result. `paste-dispatched` means dispatch, not DOM
insertion: custom/canceled native Paste is valid behavior.

`browserState(pane)` exposes payload-free diagnostics, not authorization:
`clipboard_available`, `clipboard_pending`, `clipboard_transport_busy` (global),
`clipboard_request` (pending ID or null), `clipboard_outcome`, `clipboard_error`.
Outcomes include `idle`, `pending`, `copied`, `no-update-observed`,
`paste-dispatched`, `cancelled`, `failed`, `link-copied`; unsupported builds report
unavailable with no pending request. Availability is not a completion promise;
errors contain no clipboard text. The source context token remains the one-shot
menu capability.

Normal editing aliases are Ctrl+C/X/V, Ctrl+Shift+V (plain-text Paste / MatchStyle
equivalent), Ctrl+Insert (Copy), Shift+Insert (Paste), Shift+Delete (Cut), without
Alt/Meta. Ctrl+Shift+C/X/Insert/Delete are not clipboard aliases. Explicit custom
global/layout bindings retain precedence. General new-browser shortcuts (#3),
popup/opener behavior (#6) and reopen (#7) remain deferred.

## Evidence and remaining acceptance

Parent-owned `run-browser-clipboard-5` with binary prefix `1f6e81…` passed its
listed private Wayland cases: independent-client Unicode/whitespace page/address
Paste; native page Copy/Cut export; real page-menu custom clipboardData Copy;
page Ctrl+C/V; address-menu Copy, Ctrl+X deletion and Ctrl+C; default/custom/
canceled Paste; readonly/contenteditable and real noneditable drag-selection
Copy; password/canceled Copy/Cut without OS export followed by healthy reuse;
focused activated legacy Paste and async read/readText denial; clean helper/CEF
shutdown and core stop exit 0. These are fixed owned fixtures, not desktop data.
The driver's private chrome `Input.on(change)`/trusted `setShared` observer only
synchronizes the test; it is not a product fallback. The external clipboard offer
is stopped before address Copy, avoiding a false independent-reader proof.

Later private run 10 passed empty Copy/Paste, cross-origin child-frame editing,
consented reads followed by unattended-read denial, stale pointer/tab-owner
rejection, same-pane link tabs and global key override. Run 11 added actual page
Ctrl+X/deletion, then failed the blocked-renderer queued-Paste pressure case
described above. The earlier receipt does not cover these newer changes.

Run 14 on binary SHA256
`afc801f79f77cc9594821d1c946b92efe1f6e5afa358f5a7e20132c4bf36cc34`
passed the full listed matrix without skipped empty cases. It also verifies
first page-menu external Paste, custom Cut, all seven real page chords, exact
64 KiB Copy, oversized Copy/Cut rejection with native Undo, no selection,
in-flight Paste focus/tab changes, and navigation/closed-tab menu revocation.
A following Copy is explicitly rejected Busy until blocked-renderer Paste drains.
Prior tabs, chrome/mixed splits and workspace/profile native regressions passed
on that exact binary. Independent recheck found no additional actionable defect.

Final suites: 476 native and 429 default-off unit passes (one ignored each),
30 integration passes per configuration, pixel tests and 114 script tests.
Parent results distinguish actual Wayland exchange, native semantic probes and
stubbed tests. These are owned fixtures, not universal website certification;
rich clipboard, general browser shortcuts and real-desktop testing remain outside
this delivery. Historical failed/intermediate receipts are retained honestly.
