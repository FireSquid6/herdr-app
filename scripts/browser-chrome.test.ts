import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

// Run the shipped command loop, not a reimplementation of its focus policy.
const source = readFileSync(new URL("../defaults/mods/browser-chrome/main.js", import.meta.url), "utf8")
  .replace(/^import .*;\n/gm, "")
  .replace("export default class BrowserChrome", "class BrowserChrome");

for (const owner of [null, { pane: "other", space: "other-workspace" }]) {
  test(`queued address focus does not follow a hidden/changed pane: ${JSON.stringify(owner)}`, async () => {
    let focused: { pane: string } | null = { pane: "p1" };
    let state = { url: "https://before.test" };
    let receive!: (command: string) => void;
    let focusCalls = 0;
    let value = "";
    const input = {
      on() {}, focus() { focusCalls++; }, set_selected_range() {},
      set_value(next: string) { value = next; },
      value() { return value; },
    };
    const BrowserChrome = new Function("View", "InputState", "props", "browserState", "focused", "nextCommand",
      `${source}; return BrowserChrome;`)(class {}, { new: () => input }, () => ({ pane: "p1" }),
      () => state, () => focused, () => new Promise<string>(resolve => { receive = resolve; }));
    const chrome = new BrowserChrome();
    chrome.init({}, { spawn: (run: Function) => { void run({ notify() {} }); } });
    // Promise resolution queues the continuation. The tab/workspace changes
    // before it runs, just as it can after native deliver_command_to returns.
    receive("focus_address");
    focused = owner;
    await Promise.resolve();
    expect(focusCalls).toBe(0);

    state = { url: "https://hidden-navigation.test" };
    receive("navigated");
    await Promise.resolve();
    expect(value).toBe(state.url); // Hidden navigation still updates the address.
    expect(focusCalls).toBe(0);

    focused = { pane: "p1" };
    receive("focus_address");
    await Promise.resolve();
    expect(focusCalls).toBe(1); // A current owner's explicit command still works.
  });
}

test("tab strip uses explicit pane/tab targets, shrinking minimums and native favicon assets", () => {
  const commands: unknown[] = [];
  const elements: any[] = [];
  const element = () => {
    const calls: Record<string, any[]> = {};
    let proxy: any;
    proxy = new Proxy({ calls }, { get(target, key: string) {
      if (key === "calls") return calls;
      return (...args: any[]) => {
        if (key === "when") { if (args[0]) args[1](proxy); }
        else calls[key] = args;
        return proxy;
      };
    }});
    elements.push(proxy); return proxy;
  };
  const state = { pane: "pane-a", workspace: "workspace", available: true, selected_tab: "a", tabs: [
    { id: "a", title: "Owned title", url: "https://a.test", loading: false, favicon: "browser-favicons/owned.png" },
    { id: "b", title: "Other title", url: "https://b.test", loading: true },
  ] };
  const names = ["View", "props", "browserState", "theme", "run", "div", "svg", "image", "h_flex", "v_flex", "Input", "BrowserPage"];
  const BrowserChrome = new Function(...names, `${source}; return BrowserChrome;`)(class {}, () => ({ pane: "pane-a" }),
    () => state, () => ({}), (name: string, args: any) => commands.push([name, args]),
    element, element, element, element, element, { new: element }, element);
  const chrome = new BrowserChrome();
  chrome.render();
  const find = (id: string) => elements.find(el => el.calls.id?.[0] === id);
  const cx = { stop_propagation() {} };
  expect(find("browser-tab-a").calls.min_w).toEqual([90]);
  expect(find("browser-tab-a").calls.flex_shrink).toEqual([0]);
  expect(find("browser-tab-scroll").calls.overflow_x_scrollbar).toEqual([]);
  find("browser-tab-a").calls.on_mouse_down[1]({}, cx);
  find("browser-tab-b").calls.on_mouse_up[1]({}, cx);
  expect(commands.pop()).toEqual(["browser.tab.move", { pane: "pane-a", tab: "a", index: 1 }]);
  find("browser-tab-b").calls.on_mouse_up[1]({}, cx);
  expect(commands.pop()).toEqual(["browser.tab.select", { pane: "pane-a", tab: "b" }]);
  find("browser-tab-close-b").calls.on_click[0]({}, cx);
  expect(commands.pop()).toEqual(["browser.tab.close", { pane: "pane-a", tab: "b" }]);
  find("browser-tab-new").calls.on_click[0]({}, cx);
  expect(commands.pop()).toEqual(["browser.tab.new", { pane: "pane-a" }]);

  // Drive the actual viewport measurement callback, then render its state.
  // A native measured-layout regression also checks the materialized wrapper.
  let notifications = 0;
  const measuredCx = { notify() { notifications++; } };
  find("browser-tab-viewport").calls.on_size_change[0]({width:303, height:34}, measuredCx);
  elements.length = 0;
  chrome.render();
  expect(find("browser-tab-a").calls.w).toEqual([150]);
  expect(find("browser-tab-b").calls.w).toEqual([150]);
  find("browser-tab-viewport").calls.on_size_change[0]({width:160, height:34}, measuredCx);
  elements.length = 0;
  chrome.render();
  expect(find("browser-tab-a").calls.w).toEqual([90]);
  expect(find("browser-tab-b").calls.w).toEqual([90]);
  find("browser-tab-viewport").calls.on_size_change[0]({width:160, height:34}, measuredCx);
  expect(notifications).toBe(2); // No resize/refresh loop at a stable width.
});

test("owned native-acceptance fixture queues explicit tab ownership before each page action", () => {
  const source = readFileSync(new URL("../tests/fixtures/browser-tabs-batch/main.js", import.meta.url), "utf8")
    .replace(/^import .*;\n/gm, "").replace("export default class extends View", "class Batch extends View");
  const buttons = new Map<string, () => void>();
  const element = () => {
    let id = "";
    let proxy: any;
    proxy = new Proxy({}, {get: (_, key) => (...args: any[]) => {
      if (key === "id") id = args[0];
      if (key === "on_click") buttons.set(id, args[0]);
      return proxy;
    }});
    return proxy;
  };
  const commands: unknown[] = [];
  const Batch = new Function("View", "div", "v_flex", "props", "run", `${source}; return Batch;`)(class {}, element, element,
    () => ({pane:"owned-pane",tab:"owned-b",url:"http://127.0.0.1/owned-c"}),
    (name: string, args: unknown) => { commands.push([name,args]); });
  new Batch().render();
  for (const action of ["navigate", "back", "forward", "reload", "stop"]) {
    commands.length = 0;
    buttons.get(`batch-select-${action}`)!();
    expect(commands).toEqual([
      ["browser.tab.select", {pane:"owned-pane",tab:"owned-b"}],
      [`browser.${action}`, action === "navigate" ? {pane:"owned-pane",url:"http://127.0.0.1/owned-c"} : {pane:"owned-pane"}],
    ]);
  }
  commands.length = 0;
  buttons.get("batch-new-navigate")!();
  expect(commands).toEqual([
    ["browser.tab.new", {pane:"owned-pane"}],
    ["browser.navigate", {pane:"owned-pane",url:"http://127.0.0.1/owned-c"}],
  ]);
});
