// Trusted browser chrome; clone this mod and select it with browser_chrome.
// BrowserPage is a native core-owned surface, never executable website content.
import { View, div } from "gpui-kit";
import { InputState, Input } from "gpui-base";
import { BrowserPage } from "herdr-browser";
import { browserState, props, focused, theme, run, nextCommand } from "herdr";

export default class BrowserChrome extends View {
  init(_props, cx) {
    this.editing = false;
    this.confirmStop = false;
    this.confirmTakeover = false;
    this.pageSize = { width: 230, height: 300 };
    this.addressMenu = null;
    this.input = InputState.new({ value: browserState(props()?.pane ?? "")?.url ?? "", placeholder: "Enter address" });
    this.input.on("focus", (_event, cx) => { this.editing = true; cx.notify(); });
    this.input.on("blur", (_event, cx) => { this.editing = false; this.addressMenu = null; cx.notify(); });
    this.input.on("change", (_event, cx) => {
      if (this.addressMenu) {
        this.addressMenu = null;
        run("browser.context_dismiss", { pane: props()?.pane ?? "" });
        cx.notify();
      }
    });
    this.input.on("submit", () => run("browser.navigate", { pane: props()?.pane ?? "", url: this.input.value() }));
    cx.spawn(async cx => {
      for (;;) {
        const command = await nextCommand();
        if (command === "focus_address" && focused()?.pane === (props()?.pane ?? "")) {
          this.input.focus();
          this.input.set_selected_range({ start: 0, end: this.input.value().length });
        }
        cx.notify();
      }
    });
  }
  action(id, label, enabled, callback) {
    const t = theme();
    return div().id(id).px(8).py(5).rounded(5).flex_none().opacity(enabled ? 1 : .4)
      .when(enabled, element => element.cursor_pointer().hover(style => style.bg(t.hover)).on_click(callback))
      .child(label);
  }
  contextMenu(menu, act) {
    const t = theme();
    const address = menu.source === "address";
    const link = !address && typeof menu.link === "string" ? menu.link : "";
    const canOpen = /^https?:\/\//i.test(link);
    const width = Math.min(230, this.pageSize.width);
    const x = Math.max(0, Math.min(Number.isFinite(menu.x) ? menu.x : 0, this.pageSize.width - width));
    const y = Math.max(0, Math.min(Number.isFinite(menu.y) ? menu.y : 0, this.pageSize.height - 300));
    const stop = (_event, cx) => cx.stop_propagation();
    return div().id("browser-context-backdrop").absolute().inset_0()
      .on_mouse_down("left", (_event, cx) => { cx.stop_propagation(); act("browser.context_dismiss"); })
      .on_mouse_down("right", (_event, cx) => { cx.stop_propagation(); act("browser.context_dismiss"); })
      .on_click(stop)
      .child(div().id("browser-context-menu").absolute().left(x).top(y).w(width).flex().flex_col()
        .max_h(Math.max(0, this.pageSize.height - y)).overflow_y_scroll().p(4).rounded(5)
        .bg(t.popover).border_1().border_color(t.border)
        .on_mouse_down("left", stop).on_mouse_down("right", stop).on_click(stop)
        .when(typeof menu.title === "string" && Boolean(menu.title), element =>
          element.child(div().px(8).py(3).truncate().text_color(t.muted).child(menu.title)))
        .when(Boolean(link), element =>
          element.child(div().px(8).py(3).truncate().text_color(t.muted).child(link)))
        .children([
          ["copy", "Copy", menu.can_copy === true],
          ["cut", "Cut", menu.editable === true && menu.can_cut === true],
          ["paste", "Paste", menu.editable === true && menu.can_paste === true],
          ["select_all", "Select All", true],
          ["copy_link", "Copy Link", Boolean(link)],
          ["open_link", "Open Link", canOpen],
          ["new_tab", "Open Link in New Tab", canOpen],
        ].filter(([action]) => !address || !["copy_link", "open_link", "new_tab"].includes(action))
          .map(([action, label, enabled]) => this.action(`browser-context-${action}`, label, enabled, (_event, cx) => {
          cx.stop_propagation();
          if (address) {
            const pane = props()?.pane ?? "";
            const current = browserState(pane)?.context_menu;
            const snapshot = this.addressMenu;
            const range = this.input.selected_range();
            if (!snapshot || snapshot.pane !== pane || snapshot.token !== menu.token
                || current?.source !== "address" || current.token !== menu.token
                || snapshot.value !== this.input.value() || snapshot.start !== range.start || snapshot.end !== range.end) {
              this.addressMenu = null;
              act("browser.context_dismiss");
              return;
            }
            this.addressMenu = null;
          }
          act("browser.context_action", { token: menu.token, action });
        }))));
  }
  render() {
    const t = theme();
    const pane = props()?.pane ?? "";
    const s = browserState(pane) ?? { pane, ready: false, url: "", tabs: [], phase: "Starting" };
    if (!this.editing && this.input.value() !== s.url) this.input.set_value(s.url);
    const act = (name, args = {}) => run(name, { pane, ...args });
    if (this.addressMenu && s.context_menu?.source === "address" && !this.addressMenu.token
        && s.context_menu.token !== this.addressMenu.previousToken) this.addressMenu.token = s.context_menu.token;
    if (this.addressMenu?.token && (s.context_menu?.source !== "address" || s.context_menu.token !== this.addressMenu.token))
      this.addressMenu = null;
    return div().id("browser-chrome").flex().flex_col().size_full().min_h(0).relative()
      .bg(t.background).text_color(t.foreground)
      .child(div().id("browser-tab-strip").flex().flex_none().h(34).min_w(0).border_b_1().border_color(t.border)
        .child(div().id("browser-tab-scroll").flex().flex_1().min_w(0).overflow_x_scroll()
          .children((s.tabs ?? []).map(tab => div().id(`browser-tab-${tab.id}`).flex().items_center().flex_none()
            .w(150).min_w(90).max_w(180).px(8).py(5).gap(6)
            .bg(tab.id === s.selected_tab ? t.selected : t.background)
            .on_click(() => act("browser.tab.select", { tab: tab.id }))
            .child(div().flex_1().min_w(0).truncate().child(tab.title || tab.url || "Blank"))
            .child(this.action(`browser-tab-close-${tab.id}`, "×", s.ready, (_event, cx) => {
              cx.stop_propagation(); act("browser.tab.close", { tab: tab.id });
            })))))
        .child(this.action("browser-tab-new", "+", s.ready, () => act("browser.tab.new"))))
      .child(div().id("browser-toolbar").flex().items_center().flex_none().gap(4).p(6)
        .child(this.action("browser-back", "←", s.ready, () => act("browser.back")))
        .child(this.action("browser-forward", "→", s.ready, () => act("browser.forward")))
        .child(this.action("browser-reload", "Reload", s.ready, () => act("browser.reload")))
        .child(div().id("browser-address").flex_1().min_w(0)
          .on_mouse_down("right", (_event, cx) => {
            cx.stop_propagation();
            this.editing = true;
            this.input.focus();
            const range = this.input.selected_range();
            this.addressMenu = { pane, value: this.input.value(), start: range.start, end: range.end,
              previousToken: browserState(pane)?.context_menu?.token, token: null };
            act("browser.address_context", { selection: range.start !== range.end });
          })
          .child(Input.new(this.input).w_full()))
        .child(this.action("browser-tools", s.tools_open ? "Hide DevTools" : "DevTools ↓", s.ready,
          () => act(s.tools_open ? "browser.devtools.close" : "browser.devtools.open"))))
      .child(div().id("browser-connection").flex().items_center().gap(6).px(8).py(3).text_size(11)
        .child(s.ready ? "Attached · workspace core" : (s.phase ?? "Starting"))
        .when(!s.ready, element => element
          .child(this.action("browser-attach", "Attach", true, () => act("browser.attach")))
          .child(this.action("browser-takeover", "Take over…", true, (_event, cx) => {
            act("browser.context_dismiss"); this.confirmStop = false; this.confirmTakeover = true; cx.notify();
          })))
        .when(s.ready, element => element.child(this.action("browser-stop", "Stop browser…", true, (_event, cx) => {
          act("browser.context_dismiss"); this.confirmTakeover = false; this.confirmStop = true; cx.notify();
        }))))
      .when(this.confirmStop, element => element.child(div().id("browser-stop-confirmation").flex().gap(8).p(8)
        .child("Stop Chromium? Live drafts and JavaScript memory will be lost; profile retained.")
        .child(this.action("browser-confirm-stop", "Confirm stop", true, (_event, cx) => { this.confirmStop = false; act("browser.stop", { confirmed: true }); cx.notify(); }))
        .child(this.action("browser-cancel-stop", "Cancel", true, (_event, cx) => { this.confirmStop = false; cx.notify(); }))))
      .when(this.confirmTakeover, element => element.child(div().id("browser-takeover-confirmation").flex().gap(8).p(8)
        .child("Take over Chromium? Live drafts and JavaScript memory will be lost; profile retained.")
        .child(this.action("browser-confirm-takeover", "Confirm takeover", true, (_event, cx) => {
          this.confirmTakeover = false; act("browser.attach", { force: true, confirmed: true }); cx.notify();
        }))
        .child(this.action("browser-cancel-takeover", "Cancel", true, (_event, cx) => { this.confirmTakeover = false; cx.notify(); }))))
      .when(Boolean(s.error), element => element.child(div().id("browser-error").p(8).child(s.error)))
      .child(div().id("browser-page-root").flex_1().min_w(0).min_h(0).relative().overflow_hidden()
        .on_size_change((size, cx) => { this.pageSize = size; cx.notify(); })
        .child(BrowserPage(pane))
        .when(Boolean(s.context_menu) && !this.confirmStop && !this.confirmTakeover,
          element => element.child(this.contextMenu(s.context_menu, act))));
  }
}
