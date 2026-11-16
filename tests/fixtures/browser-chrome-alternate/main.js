// Copy this folder to a PRIVATE test app home's mods/alternate-browser and set
// browser_chrome to "alternate-browser". This is not a shipped default mod.
import { View, div, svg } from "gpui-kit";
import { h_flex, v_flex, Input, InputState } from "gpui-base";
import { BrowserPage } from "herdr-browser";
import { props, browserState, run, nextCommand, theme } from "herdr";

export default class AlternateBrowser extends View {
  init(_props, cx) {
    const state = (props()?.pane ? browserState(props().pane) : null);
    this.input = InputState.new({ value: state?.url ?? "", placeholder: "Search or enter address" });
    this.input.on("submit", () => run("browser.navigate", { pane: props().pane, url: this.input.value() }));
    this.editing = false;
    this.input.on("focus", (_e, cx) => { this.editing = true; cx.notify(); });
    this.input.on("blur", (_e, cx) => { this.editing = false; cx.notify(); });
    cx.spawn(async cx => {
      for (;;) {
        const command = await nextCommand();
        if (command === "focus_address") {
          this.input.focus();
          this.input.set_selected_range({ start: 0, end: this.input.value().length });
        } else if (command === "navigated") {
          this.input.set_value((props()?.pane ? browserState(props().pane) : null)?.url ?? "");
        }
        cx.notify();
      }
    });
  }
  button(id, icon, title, enabled, action) {
    const t = theme();
    return div().id(id).size(28).flex_none().flex().items_center().justify_center()
      .rounded(6).text_color(t.muted).tooltip(title).opacity(enabled ? 1 : 0.35)
      .when(enabled, el => el.cursor_pointer().hover(s => s.bg(t.hover).text_color(t.foreground)).on_click(action))
      .child(svg(icon === "refresh" ? "frame/settings/refresh.svg" : `mods/workspace-chrome/${icon}.svg`).size(15));
  }
  render() {
    const s = browserState(props().pane);
    if (!s) return div().child("Waiting for browser");
    return v_flex().id("alternate-browser-chrome").size_full().min_h(0)
      .bg("#321044").text_color("#ffffff")
      .child(div().flex_none().p(8).child("ALTERNATE CHROME · address below the page"))
      .when(Boolean(s.error), el => el.child(div().p(8).child(s.error)))
      .child(div().flex_1().min_h(0).child(BrowserPage(s.pane)))
      .child(h_flex().flex_none().p(10).gap(8).items_center()
        .child(this.button("alternate-back", "chevron-left", "Back", s.can_back, () => run("browser.back", { pane:s.pane })))
        .child(this.button("alternate-forward", "chevron-right", "Forward", s.can_forward, () => run("browser.forward", { pane:s.pane })))
        .child(Input.new(this.input).id("alternate-address").flex_1().h(34).px(10).bg("#ffffff").text_color("#220033"))
        .child(this.button("alternate-reload", s.loading ? "x" : "refresh", s.loading ? "Stop" : "Reload", s.available !== false && !s.remote, () => run(s.loading ? "browser.stop" : "browser.reload", { pane:s.pane })))
        .child(div().id("alternate-pane-menu").px(8).child("Pane ⋯").on_click(() => run("pane.menu", {pane:s.pane}))));
  }
}
