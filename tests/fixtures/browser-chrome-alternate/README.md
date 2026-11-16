# Alternate browser chrome acceptance fixture

Use only the parent-owned private-HOME native Wayland harness. Do not launch a
bare app, touch a real app home, or bypass Chromium sandboxing.

After the harness installs the shipped mods, copy this directory to its app
home's `mods/alternate-browser` and set `browser_chrome` to `alternate-browser`
in that home's settings.json. The self-contained fixture follows the default input/command protocol
and replaces the entire layout: purple surround,
large acceptance label, native page above, address and controls below.

Acceptance hooks:

- Screenshot default and alternate at the same local fixture URL. The label,
  purple surround, and address below the native page must differ visibly.
- Edit/navigate from the bottom editor; confirm fixture pixels and input still
  work. A redirect must update the address. Configure a local homepage/search
  template to avoid external networking for this test.
- Focus the page, invoke `browser.focus_address`, type a search, and submit.
  Explicit global/layout shortcuts must still work from both page and editor.
  Return to page focus by clicking page pixels. Rendering/loading must not steal
  address/page focus.
- Exercise history: Back disabled initially, enabled after navigation; Forward
  after Back; Reload changes to Stop/loading feedback while a delayed page loads.
- Switch away/hide/zoom and return, then close/retarget the pane: retain the page
  while hidden; explicitly close on ownership removal. Compare runtime logs.
- Click Pane menu in both layouts. When integrated with issue5, use its submenu
  to split Terminal/Browser in either direction; do not add a Rust toolbar.
- Set `browser_chrome` back to `browser-chrome` to restore. A broken/missing
  selected mod must show a mod error, not execute website text as a fallback.
- Browser-free build: restore a browser pane document and check that default or
  alternate chrome still surrounds the unavailable message and Pane menu.

This fixture is not installed with the shipped defaults and implements no tabs,
website JS bridge, clipboard, or engine policy changes.
