# Browser context menus (#4, partial delivery)

The shipped trusted `browser-chrome` mod owns page/link/address menus; CEF full
and quick menus are suppressed. Website link, selection and edit-state metadata
is bounded plain data, never executable mod code or command names. Native CEF
edit flags govern copy/cut/paste eligibility (including password/readonly cases).
Rendering a menu never reads the system clipboard.

Host-issued tokens are process-wide unique and one-shot, checked against the
actual native browser, focused frame and editing generation, not merely a pane
or durable tab ID. Hidden/closed/navigating pages, frame destruction/detach,
chrome replacement and focus loss revoke menu capabilities. `sync_tabs` revokes
BEFORE swapping the native browser and shared callbacks. Opening a link in a new
tab consumes the validated capability before calling `browser.tab.new` in that
same pane; `browser.new` and workspace tabs are not substitutes.

Address menus retain their exact GPUI focus handle. Trusted chrome snapshots
value and UTF-16 selection at right-click, dismissing stale selections rather
than restoring a range into changed text. Authorized address actions dispatch
synchronously through the captured editor focus. InputState/textarea expose
`selected_range()` (UTF-16) and `set_context_menu_enabled(false)` to let trusted
mods replace the built-in menu. Normal editor Copy/Cut/Paste and explicit
custom global/layout key precedence remain intact; no extra shortcuts ship.

## Parent native clipboard integration seam

Page clipboard is TEMPORARILY FAIL-CLOSED, not a claim about stock CEF's ultimate
capabilities. `BrowserPane::edit` in `src/browser/pane.rs` is the common page
command/normal clipboard-shortcut seam; page context copy/cut/paste also call it.
Replace only its final page-unavailable branch after architecture/runtime
acceptance. `publish_chrome` currently publishes `clipboard_available: false`.
Address editing and `copy_link` already use ordinary GPUI clipboard paths and
must stay independent of page transport authorization.

The selected native page has `self.browser`, `self.shared: Arc<Callbacks>`, and
`focused_frame()` returning `(cef::Frame, context_menu::Identity { browser,
frame })`. Each inactive tab retains its OWN browser and shared callbacks.
`Callbacks::editing_generation` is per native page. `invalidate_editing()` on
Callbacks revokes queued native metadata; BrowserPane's same-named method also
revokes visible/address capabilities and synchronizes its generation. Attach
pending page clipboard cancellation here, before owner swaps, and on native
close/frame/navigation callbacks; asynchronous replies must validate the exact
native identity + generation + pending user action, never re-resolve just pane.
No page clipboard transport/IPC architecture is introduced by this delivery.

## Verification boundaries

Bun tests execute the shipped menu callbacks, verify token-only commands,
fail-closed page editing, address UTF-16 snapshot guards and real-tab menu
routing. Rust tests cover native quick/full cancellation, one-shot ownership
checks, frame/close invalidation, tab-swap-before-owner-replacement, and
browser-feature-off command failure. GPUI description tests exercise the real
QuickJS input helpers and UTF-16 selection. These are NOT page clipboard or
independent Wayland clipboard acceptance tests. Parent owns private compositor
acceptance; no production/desktop input is authorized.
