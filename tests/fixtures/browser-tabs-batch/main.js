// Owned trusted-mod fixture, not website code. Install only in the parent's
// private app home. A single real click queues both commands before rendering.
import { View, div } from "gpui-kit";
import { v_flex } from "gpui-base";
import { props, run } from "herdr";
export default class extends View {
  render() {
    const p = props(); // {pane, tab, url}; use only owned HTTP fixture URLs.
    const pair = (id, label, action) => div().id(id).h(36).w_full().child(label).on_click(() => {
      run("browser.tab.select", {pane:p.pane, tab:p.tab});
      run(`browser.${action}`, action === "navigate" ? {pane:p.pane, url:p.url} : {pane:p.pane});
    });
    return v_flex().size_full().gap(4)
      .child(pair("batch-select-navigate", "Select then Navigate (one callback)", "navigate"))
      .child(pair("batch-select-back", "Select then Back", "back"))
      .child(pair("batch-select-forward", "Select then Forward", "forward"))
      .child(pair("batch-select-reload", "Select then Reload", "reload"))
      .child(pair("batch-select-stop", "Select then Stop", "stop"))
      .child(div().id("batch-new-navigate").h(36).w_full().child("New then Navigate (one callback)").on_click(() => {
        run("browser.tab.new", {pane:p.pane});
        run("browser.navigate", {pane:p.pane, url:p.url});
      }));
  }
}
