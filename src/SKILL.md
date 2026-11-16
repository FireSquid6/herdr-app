# herdr-app: drive the person's window

Herdr shows spaces, tabs and panes. herdr-core (`hc`) runs their terminals
and keeps them alive when the window closes. Use `herdr-app` only to change
what the person sees. For starting, reading, messaging, waiting on or cleaning
up agents and programs, read `hc --skill` and use hc.
Your terminal address is `$HC_TERMINAL_ID`; it is not a pane ID.

## Inspect before acting

```sh
herdr-app ls          # spaces > tabs > panes (browser URLs) and terminals, with IDs
herdr-app status      # current layout, focus, overlays, zoom and file/mod errors
herdr-app commands    # available commands, arguments and descriptions
```

`ls` and `status` talk to the running window, not a new app. `commands`
prints the built-in command list even without an open window. Do not launch bare
`herdr-app` to inspect it: that opens or raises the window. Do not stop,
signal or restart the person's app or core. A window closing does not kill
its terminals.

`app.check_updates` checks for newer releases. Settings reports "Up to date"
only after a successful check; on failure, check the connection and retry.

## Show something when asked

New spaces and tabs are shown immediately: only create them when asked.
Pick an explicit, useful name for every new space and tab; don't rely on
default names. Use IDs from `ls` rather than guessing or acting on the wrong
focused pane.

```sh
herdr-app do space.new --name api --folder /path/to/api
herdr-app do tab.new --name "fix login" --argv '["claude"]' --task "Look at the login bug"
herdr-app do pane.split --direction right --pane p12
herdr-app do agent.focus --terminal t7
```

For a remote space:

```sh
herdr-app do space.new --name "remote api" --machine vps --folder /srv/api
```

The folder is on that machine; tabs and splits in the space stay there.
New-space keys and clicks follow the focused space's machine. Script commands
without `--machine` still use this computer; `--machine ''` explicitly chooses it.
The agent menu checks and starts agents in the remote Unix user's
login/interactive shell (sh, bash, zsh, dash, ksh or fish), including its
configured PATH. Other shells keep the sh launcher.
`terminal.open --folder /path --place down` starts the split in that folder,
including when `--argv` supplies a program. Without `--folder`, it follows the
focused terminal's folder. If an inherited local folder was deleted, new spaces,
tabs and splits retry in that machine's home. An explicit invalid folder
still fails.
`hc machine ls` lists machines. Use `hc run --on vps ...`, `hc exec --on vps ...`
and `hc file ... --on vps ...` for remote work, not your local file tools.

`do` accepts `--key value` arguments (JSON for arrays/objects) or one JSON
object. Unknown argument names fail and list valid names. Target IDs (`id`,
`pane`, `space`, `target`, `terminal`) must be strings; other non-null types fail before
the current-target default can apply. Null keeps its omitted-argument meaning.
Use `commands` to confirm the command's exact shape:

```sh
herdr-app do pane.split '{"direction":"right","pane":"p12"}'
```

`pane.swap {pane?, direction | target}` trades the places of `pane` (default:
the focused one) and its neighbor in a direction, or `target`, another pane of
the same tab. Give `direction` or `target`, not both. Focus stays with the same pane:

```sh
herdr-app do pane.swap '{"pane":"p12","target":"p14"}'
```

Browser prototype (optional Linux `browser` build, native Wayland only):
`browser.new {url?, place?: "tab" | "right" | "down", space?, pane?}` opens a
browser pane in an existing local space. `browser.navigate {url, pane?}` accepts
an HTTP(S) address or search terms.
Browser panes have independent browser tabs (not Herdr workspace tabs).
`browser.tab.new {pane?, url?}` opens the configured homepage by default;
`browser.tab.select {pane?, tab}`, `browser.tab.move {pane?, tab, index}`
(zero-based), and `browser.tab.close {pane?, tab?}` select/reorder/close them.
Closing the last closes its pane. The globally selected `browser_chrome` mod
owns the tab strip; `browserState(pane).tabs` and `.active_tab` expose it.
Loaded inactive tabs retain live state/history without hidden frame uploads.
Restart restores IDs/order/URLs/selection lazily, not forms/JS/history.
`BrowserPage(pane)` and legacy navigation commands still target the selected page.

`browser.back`, `browser.forward`, `browser.reload`, `browser.stop` and
`browser.focus_address` take optional `pane`
(default: focused browser). Configure `browser_url` (Google by default) and
`browser_search` (`https://www.google.com/search?q={query}`) in settings.json;
customize commands through keys or a mod. `browser_chrome` selects one global
trusted mod (default `browser-chrome`) that owns the toolbar, native address
editor styling, and surrounding layout. Clone the shipped mod with `mod.clone`,
edit the personal copy, and select its name in settings.json; it hot reloads.
Its props contain `pane`; `browserState(pane)` is inert data and `BrowserPage`
from `herdr-browser` composes the native page. Never execute page metadata as
mod code or command names. See the installed AGENTS.md/API declarations.
`browser.copy`, `browser.cut` and `browser.paste` take optional `pane` and target
its focused page or native address editor. Page clipboard uses stock CEF native
DOM events, including custom clipboardData/cancellation, for plain text up to
64 KiB (UTF-8 bytes); no rich HTML/image or IME/selection-mirror fallback is
promised. Page command return is asynchronous acceptance, not completion.
`browserState(pane)` exposes payload-free `clipboard_available`,
`clipboard_pending`, `clipboard_transport_busy` (app-global), `clipboard_request`,
`clipboard_outcome` and `clipboard_error` diagnostics, not action capabilities.
`copied` means OS export; `paste-dispatched` promises dispatch, not insertion.
`no-update-observed` is not proven cancellation: no OS write/success claim, but a
new explicit user command may retry. Never automatically retry Cut. Genuine
helper/seed/transport failure disables page clipboard until app restart.
Each Paste snapshots fresh GPUI text (absent != empty). One app-global serialized
helper uses an isolated ephemeral CEF context, with no profile/cookie sharing,
network/resources or grants. Text stays bounded in process memory, with no
payload logging/persistence. No immediate Paste scrub interrupts eligible
consented reads. Owner/frame/document-generation/focus checks guard native
source dispatch and OS export; tab/navigation/hide/close/focus/hot-reload changes
abandon export and drain pending helper work. Address/link copy remains separate.
Ordinary unattended website async/legacy reads, including activation outside
native Paste, are denied. Gesture writers and eligible Paste-handler reads are
normal consented behavior; `writeText()` uses CEF's private buffer, not a promised
direct website-copy-button-to-OS bridge. The OS clipboard is shared across spaces.
Normal page editing aliases are Ctrl+C/X/V, Ctrl+Shift+V (plain-text Paste),
Ctrl+Insert (Copy), Shift+Insert (Paste), Shift+Delete (Cut), without Alt/Meta.
Ctrl+Shift+C/X/Insert/Delete are not clipboard aliases. Explicit global/layout
bindings still take precedence; general browser shortcuts/popups/reopening remain
deferred.
Trusted chrome owns page/address context menus with host-issued one-shot tokens:
`browser.address_context {pane?, owner, selection?}` acquires an exact-editor-focus
and UTF-16-selection-bound address token; `browser.context_action {pane?, token,
action}` consumes a page/address action. CEF full/quick menus are canceled.
`browserState(pane).context` strings are untrusted data, never code.
`open_link_new_tab` consumes the native link capability and uses `browser.tab.new`
in the same pane, not a workspace tab. Put mouse listeners on a plain wrapper
around opaque `Input`. Never test clipboard/input on the real desktop; see
`docs/browser-context-menus.md` for the intermediate native evidence and remaining
acceptance boundaries.
Normal pane split/focus/zoom/close works. `pane.menu {pane?}` opens the
layout-selected pane menu (also on blocked/unavailable pages), preserving its
custom dimensions and missing-mod fallback.
Tab IDs/order/selection and last committed URLs are layout-persisted, not history/forms; cookies/site data
may persist in the app home's per-space `browser-profiles/`. Hidden panes stay
live; removing or retargeting a pane discards its session. Cross-space moves are
blocked. Remote spaces are blocked, never silently routed through local networking;
SSH browser routing is not implemented. Disabled/other-OS builds preserve panes
with an explanatory placeholder. Restored tab inspection/selection/reordering/closing
remain available; new native pages and page navigation report unavailable.

Mixed splitting is also discoverable in pane menus: terminal right-click (or
Shift+right-click) and the customizable browser chrome's **Pane menu** icon both offer
**Split right → Terminal / Browser** and **Split down → Terminal / Browser**.
They create a neighbor, not a tab move or content replacement. Use explicit
`pane` when scripting an unfocused target:

```sh
herdr-app do pane.split --direction down --pane p12
herdr-app do browser.new --place right --pane p12
```

Both are independently bindable (`pane.split:right` / `pane.split:down`, or typed
`browser.new` args with `place: "right"` / `"down"`). `browser.new:right` is not a
split binding: its shorthand argument is URL. Remote browser choices are visibly
blocked; browser-free builds report unavailable without disabling Terminal.
The default chrome retains Pane menu on restored browser placeholders for terminal split, zoom and close.
Both entry points respect the layout's `terminal_menu` override and its overlay
dimensions. Browser menu targets have `pane` but no `terminal`; personal menus
should guard terminal-only actions.

`terminal.search` opens a find bar in the focused terminal. It searches that
terminal's screen and scrollback; Enter and Shift-Enter move between matches,
Escape closes the bar while its find field has focus, and each terminal keeps its own query (reopening selects it, so typing replaces it).
Clicking the terminal while the bar is open puts typing, including Escape, back in the program; the bar and its matches stay.

It prints the created IDs (for example `{ "pane": "...", "terminal": "..." }`)
or `{ "error": "..." }` and exits 1. Read the result, then inspect `ls` or
`status` to confirm. Commands invoked by an agent never open a missing-argument
dialog: supply all required arguments. A successful launch reply means the
terminal was created, not that its program started correctly. If it exits
with a nonzero code within two seconds, the window shows an error toast with
the exit code and last output lines.

A terminal started with hc opts into the window with `herdr.space=<id>`.
`herdr.tab=<id>` chooses its tab; without it, Herdr creates a tab in that space.
Unlabeled workers stay hidden. Do not add labels just to run a background
worker. Use app commands when the
user wants to see a new tab or split, hc for orchestration that need not change
the screen.

## Customize layouts, keys, or mods

First read `~/.config/herdr-app/AGENTS.md` (or `$HERDR_APP_HOME/AGENTS.md` if
set). It is the installed guide to settings, layouts, commands and mods.
Read the generated `.d.ts` files beside it for full API details; do not edit
those files or the generated guide. The app rewrites them on start.

- Home defaults to `~/.config/herdr-app`; `HERDR_APP_HOME` selects another.
- Mods live in `mods/<name>/` with `main.js` and optional `mod.json`.
  They are trusted JavaScript with command and network access.
  `"focusable": true` in `mod.json` lets `pane.go` move onto a copy; it receives
  `"focus"` from `nextCommand()` and focuses its own keyboard handle.
- Shipped mods belong to Herdr: duplicate with `mod.clone` before editing.
  Shipped layouts likewise use `layout.clone` for a personal copy.
  Discover required arguments with `herdr-app commands`.
- Save to reload. A broken save keeps the old version running and reports
  an error in `status`; a still-working screen is not proof the edit worked.
- Make only the requested change. Don't touch unrelated mods or settings.
- Use theme colors. Do not retain a mod's `cx` across callbacks; each
  callback receives its own. Native animations, not redraw timers, keep
  the window cheap to run. Root `on_size_change((size, cx) => …)` reports
  logical pixels after layout, including resizes without pointer movement.
  Optional `View.close(cx)` gets one bounded final call; start cleanup there,
  not in an earlier task. The installed guide explains its 250 ms quit budget
  and which cleanup commands go out before shutdown.

After every mod change:

```sh
herdr-app check ~/.config/herdr-app/mods/<name>
herdr-app status
herdr-app shot /tmp/mod.png   # then look at the picture
```

`check` loads the mod and renders it once. It exits nonzero with the exact
error. Its shared state starts empty and stays in memory; it leaves shared.json
untouched. Never call a mod done without a passing check. Then confirm it is
shown correctly in the running window with `shot`; don't claim a visual
check you haven't made.

`herdr-app shot [file.png]` saves the window's last frame and prints its
absolute path (default: `herdr-shot.png` in your current folder). It is the
app's own rendering at full pixel size: terminals, images, overlays and
floats included, even when the window is covered or minimized. No screen
capture or permissions. It needs a running window that has drawn a frame;
`HERDR_APP_HOME` selects the window, as for `status`.

## Problems and safety

If Herdr misbehaves, `herdr-app feedback "<what happened>"` drafts a report.
The person reviews and sends it; it does not silently send feedback.

Never experiment with pointer/key automation on the person's real display.
When developing the app on Linux, its `scripts/sandbox` provides an isolated
home, core and display: `up`, `do`, `status`, `shot`, then `down` when done.
Never stop a server or session you didn't start.
