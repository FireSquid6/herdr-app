import { beforeEach, expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// Render and click the shipped view, keeping the selected pane distinct from focus.
class Element {
  nodes: any[] = [];
  calls: Record<string, any[]> = {};
  constructor() {
    const proxy: Element = new Proxy(this, {
      get: (target, key: string) => key in target ? (target as any)[key] : (...args: any[]) => {
        if (key === "child") target.nodes.push(args[0]);
        else if (key === "children") target.nodes.push(...args[0]);
        else if (key === "when") { if (args[0]) args[1](proxy); }
        else target.calls[key] = args;
        return proxy;
      },
    });
    return proxy;
  }
}
const source = await readFile(new URL("../defaults/mods/menu/main.js", import.meta.url), "utf8");
const element = () => new Element();
let properties: any, paneList: any[], spaceList: any[], terminalList: any[], keyList: any[];
const events: any[] = [];
const values = {
  View: class {}, div: element, svg: element, v_flex: element, h_flex: element,
  props: () => properties, panes: () => paneList, spaces: () => spaceList, terminals: () => terminalList,
  theme: () => ({}), keys: () => keyList, exec() {},
  run: (...args: any[]) => events.push(["run", ...args]),
  closeOverlay: (id: string) => events.push(["close", id]), openOverlay() {},
};
const Menu = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class Menu", "return class Menu"))(...Object.values(values));
const cx = { notify() {}, focus_handle: () => ({ focus() {} }), spawn() {} };
function text(node: any): string {
  return typeof node === "string" ? node : (node?.nodes ?? []).map(text).join(" ");
}
function row(node: Element, label: string): Element | undefined {
  if (node?.calls?.on_click && text(node).includes(label)) return node;
  for (const child of node?.nodes ?? []) { const found = row(child, label); if (found) return found; }
}
function click(view: any, label: string) {
  const found = row(view.render(cx), label);
  expect(found, `menu row ${label}`).toBeDefined();
  found!.calls.on_click[0]({}, cx);
}
function menu() { const view = new Menu(); view.init({}, cx); return view; }
beforeEach(() => {
  properties = { tab: "pane", target: { pane: "target", terminal: "t1" }, overlay: "popup" };
  paneList = [{ pane: "focused", tab: "other", focused: true }, { pane: "target", tab: "tab1", focused: false }];
  spaceList = [{ id: "s1", machine: null, tabs: [{ id: "tab1" }] }];
  terminalList = [{ address: "t1" }];
  keyList = [];
  events.length = 0;
});

test("Split right opens Terminal / Browser choices without closing or creating", () => {
  const view = menu();
  click(view, "Split right");
  expect(text(view.render(cx))).toContain("Terminal");
  expect(text(view.render(cx))).toContain("Browser");
  expect(events).toEqual([]);
});

for (const source of ["terminal", "browser"]) {
  for (const direction of ["right", "down"]) {
    for (const kind of ["Terminal", "Browser"]) {
      test(`${source} -> ${direction} ${kind} targets clicked pane, not focus`, () => {
        if (source === "browser") {
          properties.target = { pane: "target" };
          paneList[1].browser = true;
          terminalList = [];
        }
        const view = menu();
        click(view, `Split ${direction}`);
        click(view, kind);
        expect(events).toEqual([
          ["close", "popup"],
          ["run", kind === "Terminal" ? "pane.split" : "browser.new",
            kind === "Terminal" ? { direction, pane: "target" } : { place: direction, pane: "target" }],
        ]);
      });
    }
  }
}

test("remote browser choice is visibly blocked but terminal split still works", () => {
  for (const source of ["terminal", "browser"]) {
    for (const direction of ["right", "down"]) {
      events.length = 0;
      if (source === "browser") properties.target = { pane: "target" };
      spaceList[0].machine = "ssh-fixture";
      // The active space is intentionally local: policy must follow the target.
      spaceList.unshift({ id: "local", active: true, machine: null, tabs: [{ id: "other" }] });
      const view = menu();
      click(view, `Split ${direction}`);
      expect(text(row(view.render(cx), "Browser"))).toContain("remote blocked");
      click(view, "Browser");
      expect(events).toEqual([]);
      click(view, "Terminal");
      expect(events.at(-1)).toEqual(["run", "pane.split", { direction, pane: "target" }]);
      spaceList.shift();
    }
  }
});

test("default build does not hide terminal split behind browser availability", () => {
  // No capability API is required: browser.new is discoverable in all builds,
  // and the host reports its unavailable error through the normal command toast.
  // A rejected browser request must not poison the next terminal choice.
  const view = menu();
  click(view, "Split down");
  click(view, "Browser");
  expect(events.at(-1)).toEqual(["run", "browser.new", { place: "down", pane: "target" }]);
  events.length = 0;
  const reopened = menu();
  click(reopened, "Split down");
  click(reopened, "Terminal");
  expect(events.at(-1)).toEqual(["run", "pane.split", { direction: "down", pane: "target" }]);
});

test("keyboard enters and leaves split choices; Escape then closes root menu", () => {
  const view = menu();
  const key = (key: string) => view.render(cx).calls.on_key_down[0]({ key }, cx);
  key("right");
  expect(text(view.render(cx))).toContain("Terminal");
  key("left");
  expect(text(view.render(cx))).toContain("Split down");
  key("down"); key("enter");
  key("down"); key("enter");
  expect(events.at(-1)).toEqual(["run", "browser.new", { place: "down", pane: "target" }]);
  events.length = 0;
  const fresh = menu();
  click(fresh, "Split right");
  fresh.render(cx).calls.on_key_down[0]({ key: "escape" }, cx);
  expect(events).toEqual([]);
  fresh.render(cx).calls.on_key_down[0]({ key: "escape" }, cx);
  expect(events).toEqual([["close", "popup"]]);
});

test("Back returns without action and pane zoom/close retain explicit target", () => {
  for (const [label, command] of [["Zoom", "pane.zoom"], ["Close pane", "pane.close"]]) {
    events.length = 0;
    const view = menu();
    click(view, "Split down"); click(view, "Back");
    expect(events).toEqual([]);
    click(view, label);
    expect(events).toEqual([["close", "popup"], ["run", command, { pane: "target" }]]);
  }
});

test("layout terminals still have no model-pane controls", () => {
  properties.target.pane = "layout-terminal";
  const tree = menu().render(cx);
  for (const label of ["Split right", "Split down", "Zoom", "Close pane"]) expect(row(tree, label)).toBeUndefined();
  expect(text(tree)).toContain("Send right-clicks to pane");
});


test("split hints distinguish all four independently bound actions from new browser tab", () => {
  keyList = [
    { section: "global", keys: "tab-key", action: "browser.new" },
    { section: "global", keys: "terminal-right", action: "pane.split:right" },
    { section: "global", keys: "terminal-down", action: JSON.stringify({ command: "pane.split", args: { direction: "down" } }) },
    ...["right", "down"].map(place => ({ section: "global", keys: `browser-${place}`, action: JSON.stringify({ command: "browser.new", args: { place } }) })),
  ];
  for (const direction of ["right", "down"]) {
    const view = menu(); click(view, `Split ${direction}`);
    const tree = view.render(cx);
    expect(text(row(tree, "Terminal"))).toContain(`terminal-${direction}`);
    expect(text(row(tree, "Browser"))).toContain(`browser-${direction}`);
    expect(text(row(tree, "Browser"))).not.toContain("tab-key");
  }
});
