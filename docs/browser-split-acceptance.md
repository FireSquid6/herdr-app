# Mixed pane split acceptance (fork issue #5)

## Isolation

Run only against private app/core/profile paths and a private native-Wayland
compositor, with owned loopback HTTP fixtures. Never inject input into the real
desktop, use personal files/credentials, or stop an unrelated app/core. Never
weaken Chromium sandboxing or host policy to make a test pass.

The menu work does not change browser engine controls, profile isolation, remote
routing, default keys or personal-config migration. In the combined chrome work,
the **Pane menu** icon is trusted, customizable browser mod UI calling
`pane.menu`; there is no additional hardcoded Rust toolbar. The command uses the
layout's configured `terminal_menu`, overlay dimensions and normal fallback.

## Automated checks

```sh
bun test scripts/menu.test.ts
bun test scripts/*.test.ts
```

The menu tests execute the shipped view and its click/key callbacks, covering
both source types, both directions, both destinations, explicit targeting,
remote-disabled Browser choices, terminal availability, Back/Escape, zoom/close,
layout-only terminals and distinct typed binding hints. These tests alone do not
prove a native shell/page was created.

Native app tests also cover:

- `mixed_splits_add_neighbors_without_replacing_or_moving_existing_content`:
  model matrix, preservation, new-pane focus, zoom and close.
- `mixed_split_browser_rejections_leave_existing_content_untouched`:
  browser-free unavailable and browser-enabled remote rejection, both source
  types/directions, without starting a browser or core.
- `local_state_and_overlay_initial_descriptions`: both submenu descriptions
  through the native JS bridge.
- Browser chrome ownership, mod key contexts, normal menu selection/dimensions,
  fallback pane selection and configuration reload regressions.

The companion parent PR supplies `run-browser-ui-smoke.py` for actual private
Wayland acceptance. Private fixture mods invoke the shipped `Menu.choose`
callback for all eight source/direction/destination combinations, check newly
created pane kinds and source preservation, and capture the actual menu and
browser UI. It also verifies live global chrome replacement and restart using
an alternate mod and a distinctive pixel marker. This is callback-driven
acceptance, **not a claim of real Wayland pointer/keyboard injection**. The
existing app smoke separately checks navigation/history, profile isolation,
persistent cookies, restore and clean shutdown. Retained run results belong in
the parent PR's verification record; do not infer success from this checklist.

## Manual private-compositor checklist

- From a terminal's right-click menu (Shift+right-click when the program owns
  right-click), select Split right/down, then Terminal/Browser.
- From a browser's default Pane menu icon, perform the same choices. Confirm the
  chosen source is targeted even when another pane had focus.
- Confirm each split adds a neighbor in the source tab/workspace, selects the
  new pane and exits zoom without moving/replacing existing content.
- Resize dividers, focus, zoom and close the neighbor; preserve the original
  shell/page and normal native page interaction.
- Use Up/Down, Right/Enter, Left/Escape and Back. Entering/leaving a submenu must
  not itself create or close a pane.
- Restore a browser pane in a browser-free build. Default chrome must retain
  Pane menu access; a Browser split reports unavailable without mutation, while
  Terminal, zoom and close remain usable.
- Verify remote Browser choices are visibly blocked and direct browser.new
  fails closed. Use only owned remote fixtures if separately authorized;
  deterministic rejection tests do not require real SSH credentials.

Bind terminal splits with `pane.split:right` / `pane.split:down`, and browser
splits with typed `browser.new` args containing `place: "right"` / `"down"`.
`browser.new:right` is not a split binding: shorthand fills URL. Keep personal
keys and menu overrides intact. Custom pane menus must guard terminal-only
actions because browser menu targets have `pane` but no `terminal`.
