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
