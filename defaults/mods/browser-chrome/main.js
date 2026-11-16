// Trusted, globally selected browser chrome. Clone this mod and select it with
// settings.json's browser_chrome; the page is only a native composition slot.
import { View, div, svg, image } from "gpui-kit";
import { h_flex, v_flex, InputState, Input } from "gpui-base";
import { BrowserPage } from "herdr-browser";
import { browserState, props, focused, theme, run, nextCommand } from "herdr";

export default class BrowserChrome extends View {
  init(_props, cx) {
    const state = (props()?.pane ? browserState(props().pane) : null);
    this.input = InputState.new({ value: state?.url ?? "", placeholder: "Search or enter address" });
    this.input.set_context_menu_enabled(state?.available === false);
    this.input.on("submit", () => run("browser.navigate", { pane: props().pane, url: this.input.value() }));
    this.pressedTab = null;
    this.tabViewportWidth = 0;
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
      .child(icon === "plus" ? "+" : svg(icon === "refresh" ? "frame/settings/refresh.svg" : `mods/workspace-chrome/${icon}.svg`).size(15));
  }
  contextMenu(s) {
    const menu = s.context;
    if (!menu) return null;
    const address = menu.source === "address";
    const item = (id, label, action) => div().id(`browser-context-${id}`).px(12).py(7).cursor_pointer()
      .hover(el => el.bg(theme().hover))
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click(() => {
        // Keep the exact UTF-16 selection the address menu was opened for.
        // Do not restore a stale range into text edited while the menu was up.
        if (address && action !== "dismiss") {
          const range = this.input.selected_range();
          const snapshot = this.addressMenu;
          if (!snapshot || snapshot.owner !== s.owner || snapshot.value !== this.input.value()
              || snapshot.start !== range.start || snapshot.end !== range.end) action = "dismiss";
        }
        run("browser.context_action", { pane: s.pane, token: menu.token, action });
      }).child(label);
    const rows = [];
    if (!address && menu.link) {
      rows.push(item("open-link", "Open link", "open_link"));
      if (s.can_open_link_in_tab) rows.push(item("open-link-new-tab", "Open link in new browser tab", "open_link_new_tab"));
      rows.push(item("copy-link", "Copy link address", "copy_link"));
    }
    const editingAllowed = address || s.clipboard_available === true;
    if (editingAllowed && menu.can_copy) rows.push(item("copy", "Copy", "copy"));
    if (editingAllowed && menu.editable && menu.can_cut) rows.push(item("cut", "Cut", "cut"));
    if (editingAllowed && menu.editable && menu.can_paste) rows.push(item("paste", "Paste", "paste"));
    if (!editingAllowed) rows.push(div().id("browser-context-clipboard-unavailable").px(12).py(7).text_color(theme().muted).child("Page clipboard unavailable"));
    rows.push(item("select-all", "Select all", "select_all"));
    if (!address) {
      if (s.can_back) rows.push(item("back", "Back", "back"));
      if (s.can_forward) rows.push(item("forward", "Forward", "forward"));
      rows.push(item("reload", "Reload", "reload"));
    }
    return v_flex().id("browser-context-menu").absolute().top(address ? 42 : 80).right(8).w(240)
      .bg(theme().chrome).border_1().border_color(theme().border).rounded(8).py(4)
      .on_mouse_down_out(() => run("browser.context_action", { pane: s.pane, token: menu.token, action: "dismiss" }))
      .children(rows);
  }
  render() {
    const t = theme();
    const s = (props()?.pane ? browserState(props().pane) : null);
    if (!s) return div().child("Browser page unavailable");
    const action = name => () => run(`browser.${name}`, { pane: s.pane });
    const allowed = s.available !== false && !s.remote && !s.closed;
    const count = s.tabs?.length ?? 0;
    // The scrollbar's native content wrapper is auto-width/nonshrinking.
    // Give its tabs explicit widths based on the *outer* measured viewport,
    // rather than expecting shrink on that intrinsic-width content row.
    const gaps = Math.max(0, count - 1) * 3;
    const tabWidth = count ? Math.max(90, Math.min(180, ((this.tabViewportWidth || count * 180 + gaps) - gaps) / count)) : 180;
    return v_flex().id("browser-chrome").relative().size_full().min_h(0).bg(t.background).text_color(t.foreground)
      .child(h_flex().id("browser-tab-strip").flex_none().min_w(0).h(34).items_center().gap(3).bg(t.chrome)
        .on_mouse_up("left", () => { this.pressedTab = null; })
        .child(div().id("browser-tab-viewport").flex_1().min_w(0).h_full().overflow_hidden()
          .on_size_change((size, cx) => {
            if (this.tabViewportWidth !== size.width) { this.tabViewportWidth = size.width; cx.notify(); }
          })
          .child(h_flex().id("browser-tab-scroll").w_full().min_w(0).h_full().items_center().gap(3).overflow_x_scrollbar()
          .children((s.tabs ?? []).map((tab, index) => h_flex().id(`browser-tab-${tab.id}`)
            .w(tabWidth).min_w(90).flex_shrink(0).h(29).px(8).gap(5).items_center().rounded(6)
            .bg(tab.id === s.selected_tab ? t.raised : t.chrome).text_size(12)
            .on_mouse_down("left", () => { this.pressedTab = tab.id; })
            .on_mouse_up("left", (_e, cx) => {
              cx.stop_propagation();
              const from = this.pressedTab; this.pressedTab = null;
              if (s.closed) return;
              if (from && from !== tab.id) run("browser.tab.move", { pane: s.pane, tab: from, index });
              else run("browser.tab.select", { pane: s.pane, tab: tab.id });
            })
            .when(Boolean(tab.favicon), el => el.child(image(tab.favicon).size(14).flex_none()))
            .when(tab.loading, el => el.child(svg("mods/workspace-chrome/spinner.svg").size(12).flex_none().spin(900)))
            .child(div().flex_1().min_w(0).truncate().child(tab.title || tab.url || "New tab"))
            .child(div().id(`browser-tab-close-${tab.id}`).flex_none().cursor_pointer()
              .on_mouse_down("left", (_e, cx) => { cx.stop_propagation(); this.pressedTab = null; })
              .on_mouse_up("left", (_e, cx) => cx.stop_propagation())
              .on_click((_e, cx) => { cx.stop_propagation(); if (!s.closed) run("browser.tab.close", { pane: s.pane, tab: tab.id }); })
              .child(svg("mods/pane/close.svg").size(12)))))))
        .child(this.button("browser-tab-new", "plus", "New tab", allowed, action("tab.new"))))
      .child(h_flex().id("browser-toolbar").flex_none().p(6).gap(4).items_center().bg(t.chrome)
        .child(this.button("browser-back", "chevron-left", "Back", allowed && s.can_back, action("back")))
        .child(this.button("browser-forward", "chevron-right", "Forward", allowed && s.can_forward, action("forward")))
        .child(this.button("browser-reload-stop", s.loading ? "x" : "refresh", s.loading ? "Stop" : "Reload", allowed, action(s.loading ? "stop" : "reload")))
        // Shell mouse listeners attach only to plain elements, not opaque Input.
        .child(div().id("browser-address-context").flex_1().min_w(0)
          .when(allowed, el => el.on_mouse_down("right", (_e, cx) => {
            cx.stop_propagation();
            this.input.focus();
            const range = this.input.selected_range();
            this.addressMenu = { owner: s.owner, value: this.input.value(), ...range };
            run("browser.address_context", { pane: s.pane, owner: s.owner, selection: range.start !== range.end });
          }))
          .child(Input.new(this.input).id("browser-address").w_full().min_w(0).h(30).px(10).rounded(8)
            .border_1().border_color(this.editing ? t.accent : t.border).bg(t.input).text_color(t.foreground).text_size(13)))
        .child(this.button("browser-pane-menu", "layers", "Pane menu", true,
          () => run("pane.menu", { pane: s.pane })))
        .when(s.loading, el => el.child(svg("mods/workspace-chrome/spinner.svg").size(14).flex_none().text_color(t.accent).spin(900).tooltip("Loading…"))))
      .child(div().flex_none().px(10).py(4).text_size(11).text_color(t.dim)
        .child(`Workspace: ${s.workspace} · ${s.remote ? "Remote route blocked" : "Local networking · localhost is this computer"}`))
      .when(Boolean(s.error), el => el.child(div().p(12).text_size(13).child(s.error)))
      .when(Boolean(s.clipboard_error), el => el.child(div().id("browser-clipboard-error").px(10).py(4)
        .text_size(11).text_color(t.muted).child(`Clipboard: ${s.clipboard_error}`)))
      .child(div().flex_1().min_h(0).overflow_hidden().child(BrowserPage(s.pane)))
      .when(Boolean(s.context), el => el.child(this.contextMenu(s)));
  }
}
