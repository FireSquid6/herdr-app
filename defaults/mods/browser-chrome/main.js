// Trusted, globally selected browser chrome. Clone this mod and select it with
// settings.json's browser_chrome; the page is only a native composition slot.
import { View, div, svg } from "gpui-kit";
import { h_flex, v_flex, InputState, Input } from "gpui-base";
import { BrowserPage } from "herdr-browser";
import { browserState, props, focused, theme, run, nextCommand } from "herdr";

export default class BrowserChrome extends View {
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
          // Delivery is asynchronous: the owner may have been hidden or left
          // since the request was queued. Never focus an inactive pane's input.
          if (focused()?.pane !== props().pane) continue;
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
    const t = theme();
    const s = (props()?.pane ? browserState(props().pane) : null);
    if (!s) return div().child("Browser page unavailable");
    const action = name => () => run(`browser.${name}`, { pane: s.pane });
    const allowed = s.available !== false && !s.remote && !s.closed;
    return v_flex().id("browser-chrome").size_full().min_h(0).bg(t.background).text_color(t.foreground)
      .child(h_flex().id("browser-toolbar").flex_none().p(6).gap(4).items_center().bg(t.chrome)
        .child(this.button("browser-back", "chevron-left", "Back", allowed && s.can_back, action("back")))
        .child(this.button("browser-forward", "chevron-right", "Forward", allowed && s.can_forward, action("forward")))
        .child(this.button("browser-reload-stop", s.loading ? "x" : "refresh", s.loading ? "Stop" : "Reload", allowed, action(s.loading ? "stop" : "reload")))
        .child(Input.new(this.input).id("browser-address").flex_1().min_w(0).h(30).px(10).rounded(8)
          .border_1().border_color(this.editing ? t.accent : t.border).bg(t.input).text_color(t.foreground).text_size(13))
        .child(this.button("browser-pane-menu", "layers", "Pane menu", true,
          () => run("pane.menu", { pane: s.pane })))
        .when(s.loading, el => el.child(svg("mods/workspace-chrome/spinner.svg").size(14).flex_none().text_color(t.accent).spin(900).tooltip("Loading…"))))
      .child(div().flex_none().px(10).py(4).text_size(11).text_color(t.dim)
        .child(`Workspace: ${s.workspace} · ${s.remote ? "Remote route blocked" : "Local networking · localhost is this computer"}`))
      .when(Boolean(s.error), el => el.child(div().p(12).text_size(13).child(s.error)))
      .child(div().flex_1().min_h(0).overflow_hidden().child(BrowserPage(s.pane)));
  }
}
