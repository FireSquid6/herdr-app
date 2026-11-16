# Mixed pane split acceptance (fork issue #5)

## Isolation and verification ownership

Run only against a private app home, core session, browser profile directory and
headless native-Wayland compositor. Do not send input to a real desktop. Use an
owned local HTTP fixture instead of a production website or authenticated page.
The parent owns Cargo/native verification and the existing native-Wayland
acceptance harness; do not run another Cargo build against its cache.

This change does not modify browser engine controls, profile isolation, remote
routing policy, default keys or personal-config migration. Browser pane actions
live in the app wrapper, outside both the web surface and native navigation bar.

## Fast automated checks

From this child app checkout:

```sh
source /home/firesquid/.local/share/herdr-fork-tools/env.sh
bun test scripts/menu.test.ts
bun test scripts/*.test.ts
```

The menu tests execute the shipped view and actual click/key handlers, without a
core, browser or desktop. They cover both source types, both directions, both
new-pane types, explicit targeting, remote-disabled choices, dispatch independent
of browser capability, Back/Escape, zoom/close targeting, layout-only terminals,
and distinct typed keybinding hints. They do not assert that a native shell or
page has actually been created.

Native regression cases, to run in the parent's serial native verification:

- `mixed_splits_add_neighbors_without_replacing_or_moving_existing_content`
  (feature-independent model matrix, including existing browser tab preservation,
  new-pane focus/zoom state and close).
- `mixed_split_browser_rejections_leave_existing_content_untouched`
  (window command boundary, browser-free unavailable and browser-enabled remote
  rejection; each source type and direction, no core/page/network needed).
- `local_state_and_overlay_initial_descriptions`
  (includes both split submenu descriptions through the real native JS bridge).
- The normal native suite, default build and optional Linux `browser` build.
- `herdr-app check <private-app-home>/mods/menu` in the sandbox. A native mod
  check and screenshots have **not** been run by the implementation worker.

## Demo / visual acceptance

1. In the browser-enabled scratch app, create a local space with a terminal,
   then open an owned fixture browser tab. Record `herdr-app ls` before splitting
   so the existing browser ID/URL can be checked for preservation.
2. On the terminal, right-click (Shift+right-click if a program owns the mouse).
   Select **Split right**, then **Browser**. Expect a *new* browser neighbor in
   that same tab/space, the terminal unchanged, and the preexisting browser tab
   still in its original tab. Save a screenshot of the direction submenu and
   mixed result.
3. On the new browser, click **Pane ⋯**, the small app-owned strip above its
   navigation bar. Select **Split down**, then **Terminal**. Expect a shell
   beneath that browser, no page replacement and no tab move. Save a screenshot.
4. Repeat all eight combinations in fresh scratch tabs: source Terminal/Browser,
   direction right/down, new Terminal/Browser. Before opening a source's menu,
   focus a different visible pane; the split must still be adjacent to the
   source, not the previously focused pane. Each new pane receives keyboard
   focus. `ls` confirms the new pane belongs to the original tab and space.
5. Drag each new divider; focus each pane; use Pane ⋯ or terminal menu **Zoom**
   and **Close pane**. Splitting a zoomed source exits zoom; closing only the
   neighbor preserves the original shell/page. Confirm the fixture page remains
   interactive below the app-owned action strip and native navigation bar.
6. Exercise keyboard menus: Up/Down, Right or Enter into a direction, Left or
   Escape back, Enter on Terminal/Browser; Escape at the root dismisses. Opening
   a submenu or returning with Back must never create or close a pane.
7. On the browser-free build, restore a scratch browser pane saved by the feature
   build. Its unavailable placeholder still has **Pane ⋯**; Terminal splitting,
   zoom and close remain usable. Choosing Browser from either source reports
   **Browser unavailable** through the standard command-error notification and
   does not change existing panes. A subsequent Terminal split still succeeds.
8. Use only an owned remote fixture if one is already available; otherwise the
   native rejection test above supplies the fail-closed remote check. The
   target's Browser row must say **remote blocked**, including when the active
   or previously focused space is local. Clicking it does nothing; Terminal
   splitting still targets the remote machine. A direct remote `browser.new`
   command must fail closed with no local browser/profile/network creation.

For independent shortcut acceptance, merge four scratch global bindings as shown
in `defaults/AGENTS.md`. Terminal uses `pane.split:right` / `pane.split:down`;
browser uses typed `browser.new` args `{ "place": "right" }` / `"down"`.
Do not use `browser.new:right`: shorthand fills URL. All bindings target the
focused pane unless an explicit `pane` is included. No shipped key changes are
needed. Test untouched defaults and an edited personal `terminal_menu`/keys file:
the latter remain personal. Browser Pane ⋯ must open the configured menu with
its custom overlay dimensions and `target.pane` (no terminal target). Guard
terminal-only actions in the custom menu. An unknown configured mod must fall
back to the shipped menu through the normal menu-opening path.
