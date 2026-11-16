# Browser tabs private acceptance recipe

Never launch against the production desktop/core/profile. The parent integration
harness owns a private native Wayland compositor, HOME/XDG, core socket and input.
Serve `tests/fixtures/browser-tabs/` from an owned loopback HTTP server with access
logs. Set the private settings `browser_url` to that owned server's `/index.html`
before testing new-tab homepage defaults (do not accidentally use Google).
No credentials, paid services, remote workspace browsing or sandbox bypass.

## Public seams

- `browser.new` still creates a **pane** (including mixed right/down splits).
- `browser.tab.new {pane?,url?}`, `browser.tab.select {pane?,tab}`,
  `browser.tab.close {pane?,tab?}`, `browser.tab.move {pane?,tab,index}` (zero-based).
  These never move tabs across panes. Closing the last delegates to `pane.close`.
- `herdr-app ls` browser panes include durable `browser_tabs` and live
  `browser_state`. The latter is also `browserState(pane)` in trusted chrome.
  `active_tab` (alias `selected_tab`) identifies the selected page. Each tab exposes `id`, `url`,
  `title`, `favicon` (local PNG asset, never remote URL), `loading`, `loaded`,
  `visible`, `browser_id`, `frame_copies`, `frame_uploads`.
- `BrowserPage(pane)` and the existing BrowserPane navigation/focus methods still
  target the selected page. Native page identities (`browser_id`) must survive
  selection. Any context-menu/async action must capture actual CEF browser/frame
  identity and pending authorization; **pane alone is not an action owner**.
- `BrowserEvent::Navigated {tab,url}` identifies which retained page navigated;
  the window also verifies pane entity/workspace/machine before saving it. Closed
  tab IDs have no model recipient. IDs are scoped to the owning pane; legacy
  single-URL panes use stable `initial`, new tabs use random `bt…` IDs.

## Native checks (parent-owned)

1. Open `index.html` in A. Capture tab ID/browser_id. Change `#draft`, increment
   JS twice. The owned fixture reports instance/counter/form value in its title
   on input, increment, visibility change and the **Report live state** button;
   inspect `browser_state.tabs[].title` (no privileged website-to-host bridge).
   Private CEF instrumentation may also read `window.instance/window.counter`. Create B at `other.html`.
   Return to A: browser_id, instance, form and counter unchanged. Navigate A via
   its history link, switch B/A, then Back/Forward; verify history is per-page.
2. While B is visible, capture A's counters twice separated by several paint
   intervals. A's frame_uploads must remain unchanged (copies may finish one
   already-running callback at hide). B's upload count must grow. Hide the whole
   workspace and repeat for **all** tabs. On switching back the page is live.
3. Verify both fixture titles and local decoded favicon PNGs in the strip;
   HTTP logs show favicon requests only from the workspace browser route. Closing
   a tab while its favicon is pending must not update another page. Different
   workspace contexts retain baseline cookie/storage isolation; tabs and panes
   in one workspace share its RequestContext/profile.
4. Create/reorder/select tabs, restart under the same private app home. Verify
   IDs/order/URLs/selection and surrounding mixed layout. Access logs must show
   only selected tabs in visible panes loading initially; hidden pane tabs and
   inactive tabs load only when selected. Live JS/forms/history are not restored.
5. Restore a URL-only browser layout, preserve saved URL and all neighboring
   layout data. Add a tab, save, restart and inspect the migrated document.
6. Close inactive, selected and last tabs. Last close returns pane space to its
   terminal neighbor (or closes the enclosing workspace tab if its only pane).
   Explicit pane/workspace close closes **all** retained pages, not just selected.
   Exit must log OnBeforeClose for every loaded browser and clean CEF shutdown.
7. Narrow the pane: tabs shrink to 90px minimum then scroll horizontally, never
   wrap. Verify title truncation, loading, active state, close and New Tab;
   drag between tabs reorders. Clone/select an alternate BrowserChrome mod and
   verify no Rust toolbar is substituted. Existing native focus/global shortcut
   guards, unavailable/remote Pane menu, and mixed splits remain functional.

Pure/description tests do not prove native JS survival, native history,
RequestContext egress, hidden paint suppression, favicon rendering or shutdown.
Record those separately from Cargo/Bun pass counts.

## Review regression acceptance

- Close a browser tab when it is the **only** pane of the only workspace tab.
  The browser-only space must actually disappear, including default-off restores;
  an unrelated terminal/workspace must stay alive. Fixed/nonclosable panes and
  unavailable documents must return errors with the original durable tabs intact.
  Unconfirmed ordinary `space.close` on a terminal space must still fail.
- For exact no-intervening-render ownership checks, install the owned trusted mod
  `tests/fixtures/browser-tabs-batch/` as `mods/browser-tabs-batch/` in the private
  app home. Open it as a mod pane beside the browser using the existing command
  with actual owned pane/tab IDs and the owned server URL:

  ```sh
  herdr-app do pane.split '{"direction":"down","pane":"BROWSER_PANE","mod":"browser-tabs-batch","props":{"pane":"BROWSER_PANE","tab":"TARGET_BROWSER_TAB","url":"http://127.0.0.1:PORT/page/new"}}'
  ```

  No website receives these trusted-mod commands or app socket access. A single
  real click queues select/new immediately followed by Navigate/Back/Forward/
  Reload/Stop. Observe actual native `browser_id`, fixture title/JS instance and
  HTTP events: only the selected target may navigate or receive history actions;
  the former page's JS instance/form/history must not change. An unloaded selected
  tab must not borrow the formerly selected page's native frame. Keep hidden
  panes hidden/uncreated while reconciling model-only operations.
- Measure native tab widths at a wide viewport (180px), an intermediate viewport
  (between 90 and 180px), and below the total minimum width (90px with overflow).
  New Tab stays outside the scrolling viewport. The offscreen Rust regression
  instruments **the shipped mod and real native Scrollable wrapper** via test-only
  `on_size_change` observers, not a builder-method spy or alternate flex engine.
- Navigate valid favicon → no favicon, failed favicon, and a slow old favicon
  reply arriving after a new document. The old asset must be retired and cannot
  be resurrected. App logs now distinguish candidate callbacks, current/stale
  download replies, PNG conversion and asset writes. HTTP 200 alone is not proof
  of an icon; successful native favicon rendering is recorded below.

## Recorded manager acceptance

The manager ran the immutable reviewed native feature binary, SHA256
`5ef16a7a37912dcd05499b973c7d4db6a8e3d88e6b50364a6aa08ca362bf5f70`, in the
private Wayland harness. Evidence is retained outside the repository under
`$P = /home/firesquid/.cache/herdr-browser-prototype`; these are local acceptance
receipts, not repository fixtures or a requirement to use that path elsewhere.

- **`$P/run-browser-tabs-5/browser-tabs-result.json`** reports success for every
  recorded tab/input flag: real native pointer focus and typing, live form/JS
  retention, per-tab history, two independent panes, stopped hidden uploads,
  configured homepage, restored order/selection, lazy restore, workspace profile
  sharing and isolation, last-tab pane closure, sole-browser workspace closure,
  favicon pixels and two orderly CEF shutdowns. Core stop exited 0.
- **`$P/run-browser-tabs-7/browser-tabs-result.json`** repeats those successful
  checks and adds `queued_select_new_navigation=true`. With the optional
  `--batch-mod` pointing at this repository's owned batch fixture, real private
  pointer clicks queued select-Two + Navigate and new + Navigate in single
  callbacks. The private fixture copy changed only markers/new URLs. Each target
  HTTP document and title committed; former One's native ID, URL and JS instance
  were unchanged, and former Two's ID/URL were unchanged after the new-tab action.
  This proves these two queued navigation cases, not actual-input coverage of
  every Back/Forward/Reload/Stop batch button.
- The manager reviewed **`$P/run-browser-tabs-7/tabs-retained.png`** (and run 5's
  corresponding capture): actual amber favicons and retained textarea value
  `zq` were visible. Successful native favicon installation/rendering is thus
  established, separately from HTTP success and the pure lifecycle tests.
- **`$P/run-browser-tabs-chrome-regression-2/browser-ui-result.json`** reports all
  eight mixed-split callback combinations, live custom-chrome reload, persistence
  after restart, two clean CEF shutdowns and core stop 0. Its scope is **callback
  invocation, not pointer injection**. The initial attempt without a compositor
  seat failed startup; the corrected run used `$P/weston-seat/weston-seat.so`.
  That harness setup failure was not a source regression.
- The independent reviewer rechecked all four reported fixes: transactional final
  close, immediate selected-page ownership, measured shrink-before-overflow and
  favicon lifecycle/success-path handling. All were accepted, including the
  requirement that frame diagnostics do not refresh chrome JS per native paint.

The exact-source automated receipts remain: native **447 unit + 30 integration**
passes, default-off **425 unit + 30 integration** passes (two ignored tests per
suite), both offscreen pixel harnesses passing, and **107 Bun tests** passing.
Native typecheck/build, generated-doc/CLI checks and whitespace checks passed.
These automated results complement, rather than replace, the native evidence.

This acceptance covers browser tabs and the existing customizable chrome. It does
**not** claim completion of the separate clipboard/context-menu feature, successful
clipboard header policy, or hard JavaScript API isolation. Clipboard work and its
infrastructure-only helper proofs are not included in this feature commit.
