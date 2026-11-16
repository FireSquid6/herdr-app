# Layouts and mods

You can change the whole window, not just its colors. Layouts arrange it; mods draw the panels inside it.

Read the [layout and mod guide](../defaults/AGENTS.md). This is the same guide Herdr writes into your app home on every start, so people and agents work from one source of truth.
It covers layouts, settings, keys, overlays, mod code and the app API.
The generated `herdr.d.ts` and `gpui-kit.d.ts` beside your installed guide describe the full API for your version.

Start with Settings for fonts, colors and sounds.
For deeper changes, duplicate a shipped layout or mod from the command palette, then edit the copy.
Shipped files update on start; personal copies do not.
Save to reload. Check for errors, even when the old screen still looks fine.

```sh
herdr-app check ~/.config/herdr-app/mods/my-panel
herdr-app status
```

The check loads a mod and renders it once. It does not replace looking at the result in the window.

The shipped `menu` offers two-level mixed split choices from a terminal's context
menu or a browser's **Pane ⋯** button. A personal terminal menu can do the same
using its explicit `props().target.pane` with `pane.split {direction, pane}` or
`browser.new {place: "right" | "down", pane}`. Do not substitute the focused pane
or use `browser.new:right` (that shorthand fills URL). Keep Terminal available
when Browser is unavailable or remote-blocked; the host enforces browser policy.
Browser's pane button is independent of native navigation chrome and respects
the layout's `terminal_menu` customization, including the mod's overlay width
and height. Browser targets have no `target.terminal`; guard terminal-only items.

Mods can run programs, read files and use the network. Only install code you trust.
See [security and privacy](security.md).

[Back to Herdr](../README.md)
