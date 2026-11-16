// Review regressions: restored remote pages never initialize CEF in fake GPUI.
use serde_json::json;

const REVIEW_CHROME: &str = r#"
import { View, div } from 'gpui-kit';
import { Input, InputState } from 'gpui-base';
import { BrowserPage } from 'herdr-browser';
import { props } from 'herdr';
export default class extends View {
  init() { this.input = InputState.new({value: 'https://example.com'}); }
  render() { return div().size_full().flex().flex_col()
    .child(Input.new(this.input).h(40).w_full())
    .child(div().flex_1().child(BrowserPage(props().pane))); }
}
"#;

fn review_browser_window(cx: &mut TestAppContext) -> (gpui::Entity<super::AppWindow>, &mut gpui::VisualTestContext) {
    cx.update(gpui_base::init);
    std::fs::create_dir_all(crate::mods::app_dir()).unwrap();
    std::fs::write(crate::settings::path(), r#"{"browser_chrome":"review-chrome"}"#).unwrap();
    let (view, cx) = open_window(cx, "review-chrome", REVIEW_CHROME, json!({
        "id":"main", "type":"block", "content":{"tab":"active"}
    }));
    view.update_in(cx, |this, window, cx| {
        this.model.add_space("s", "Restored remote", "", super::Tab::with_pane("tab", "Browser", super::Node::block("page", super::Content::Browser { url: "https://example.com".into(), tabs: Default::default() })));
        this.model.space_mut("s").unwrap().machine = Some("remote".into());
        this.model.split_with("page", super::Axis::Row, super::Node::block("term", super::Content::Terminal { terminal: "remote/t1".into() }));
        this.model.focus_pane("term");
        this.sync(window, cx);
        cx.notify();
    });
    cx.run_until_parked();
    (view, cx)
}

#[test]
fn settings_reload_reconciles_existing_browser_chrome() {
    in_own_process("browser_chrome_settings_reload");
}

#[gpui::test]
fn browser_chrome_settings_reload(cx: &mut TestAppContext) {
    if !running_alone("browser_chrome_settings_reload") { return; }
    let (view, cx) = review_browser_window(cx);
    // A second tab has already been shown, then hidden: its copy is owned too.
    view.update_in(cx, |this, window, cx| {
        this.model.add_tab("s", super::Tab::with_pane("hidden", "Hidden", super::Node::block("hidden-page", super::Content::Browser { url: "https://example.org".into(), tabs: Default::default() })));
        this.model.select_tab("hidden");
        this.sync(window, cx);
        this.model.select_tab("tab");
        this.sync(window, cx);
        assert_eq!(this.copies["page#browser"].inst.name, "review-chrome");
        assert!(this.copies["hidden-page#browser"].root.is_some());
    });
    for name in ["missing-chrome", "review-chrome"] {
        std::fs::write(crate::settings::path(), json!({"browser_chrome":name}).to_string()).unwrap();
        view.update_in(cx, |this, window, cx| {
            // Avoid filesystem timestamp granularity; still exercise the real reload path.
            this.watched.settings = None;
            this.reload_config(window, cx);
            let copy = &this.copies["page#browser"];
            assert_eq!(copy.inst.name, name, "reload must not wait for a tab switch");
            assert_eq!(copy.root.is_some(), name == "review-chrome");
            assert!(!this.copies.contains_key("hidden-page#browser"), "stale hidden copies must be retired without starting hidden replacements");
            assert_eq!(this.model.active_space().unwrap().active_tab().unwrap().id, "tab");
        });
        cx.run_until_parked();
    }
}

#[cfg(all(feature = "browser", target_os = "linux"))]
#[test]
fn native_browser_chrome_keeps_selected_mod_keys_and_explicit_precedence() {
    in_own_process("browser_chrome_mod_keys");
}

#[cfg(all(feature = "browser", target_os = "linux"))]
#[gpui::test]
fn browser_chrome_mod_keys(cx: &mut TestAppContext) {
    if !running_alone("browser_chrome_mod_keys") { return; }
    let (view, cx) = review_browser_window(cx);
    std::fs::write(crate::keys::path(), json!({
        "mod:review-chrome": {"ctrl-shift-y":"pane.zoom", "ctrl-shift-g":"pane.close", "ctrl-shift-l":"pane.close"},
        "global": {"ctrl-shift-g":"pane.zoom"},
        "layout:test": {"ctrl-shift-l":"pane.zoom"}
    }).to_string()).unwrap();
    view.update_in(cx, |this, window, cx| {
        this.watched.keys = None;
        this.reload_config(window, cx);
        // A stale/unvalidated selection must not relabel the copy actually mounted.
        this.person.browser_chrome = "not-loaded".into();
        cx.notify();
    });
    cx.run_until_parked();
    view.update(cx, |this, _| this.person.browser_chrome = "review-chrome".into());
    let bounds = view.update(cx, |this, _| this.bounds.borrow()["page"]);
    // Focus the ordinary Input inside the mod mounted by BrowserPane (native
    // feature) or render_slot (feature off). No page process is needed.
    cx.simulate_click(point(bounds.left() + px(20.), bounds.top() + px(20.)), Default::default());
    cx.run_until_parked();
    for (key, zoomed) in [("ctrl-shift-y", true), ("ctrl-shift-g", false), ("ctrl-shift-l", true)] {
        cx.simulate_keystrokes(key);
        cx.run_until_parked();
        view.update(cx, |this, _| {
            let tab = this.model.tab_of("page").expect("explicit shortcuts must not run the mod's pane.close");
            assert_eq!(tab.zoomed.as_deref(), zoomed.then_some("page"), "{key}");
            assert!(tab.root.find("term").is_some());
        });
    }
}

#[test]
fn clicking_browser_fallbacks_targets_the_real_pane() {
    in_own_process("browser_fallback_selection");
}

#[gpui::test]
fn browser_fallback_selection(cx: &mut TestAppContext) {
    if !running_alone("browser_fallback_selection") { return; }
    let (view, cx) = review_browser_window(cx);
    // Normal remote/feature-off view, failed mod, and absent copy placeholder.
    for branch in ["chrome", "error", "absent"] {
        view.update_in(cx, |this, window, cx| {
            this.model.focus_pane("term");
            this.sync(window, cx);
            if branch == "error" {
                let copy = this.copies.get_mut("page#browser").unwrap();
                copy.root = None;
                copy.error = Some("invalid chrome fixture".into());
            } else if branch == "absent" {
                this.remove_copy("page#browser", window, cx);
                #[cfg(all(feature = "browser", target_os = "linux"))]
                {
                    if let Some(slot) = this.browsers.remove("page") {
                        slot.view.update(cx, |view, cx| view.close(window, cx));
                    }
                    // Exercise the no-runtime placeholder, not another native
                    // remote view recreated by prepare_browsers during render.
                    this.runtime = None;
                }
            }
            cx.notify();
        });
        cx.run_until_parked();
        let at = view.update(cx, |this, _| this.bounds.borrow()["page"].center());
        cx.simulate_click(at, Default::default());
        cx.run_until_parked();
        view.update_in(cx, |this, window, cx| {
            assert_eq!(this.model.focused_pane().as_deref(), Some("page"), "{branch}");
            assert_eq!(this.focused_pane.as_deref(), Some("page"), "{branch}");
            let reply = this.run_as("pane.menu", json!({}), false, window, cx);
            assert!(reply.get("error").is_none(), "{reply}");
            let overlay = this.overlays.last().unwrap();
            assert_eq!(this.copies[&overlay.slot].inst.props["target"]["pane"], "page", "{branch}");
            let id = overlay.id.clone();
            this.close_overlay(&id, window, cx);
        });
    }
    view.update_in(cx, |this, window, cx| {
        // Supply the synthetic document's mutation permission, not a real
        // machine/connection. The browser has no terminal to kill, and this
        // disconnected slot cannot schedule remote document writes.
        this.documents.entry("remote".into()).or_default().available = true;
        let reply = this.run_as("pane.close", json!({}), false, window, cx);
        assert!(reply.get("error").is_none(), "{reply}");
        assert!(this.model.tab_of("page").is_none());
        assert!(this.model.tab_of("term").is_some(), "untargeted close must leave the neighboring terminal alive");
    });
}

#[cfg(all(feature = "browser", target_os = "linux"))]
#[test]
fn browser_tab_commands_preserve_mixed_layout_and_last_close() {
    in_own_process("browser_tab_model_commands");
}

#[cfg(all(feature = "browser", target_os = "linux"))]
#[gpui::test]
fn browser_tab_model_commands(cx: &mut TestAppContext) {
    if !running_alone("browser_tab_model_commands") { return; }
    let (view, cx) = review_browser_window(cx);
    view.update_in(cx, |this, window, cx| {
        // Never instantiate a local CEF page in a fake display. Exercise the
        // public commands against a locally owned model with runtime detached.
        for (_, slot) in this.browsers.drain() { slot.view.update(cx, |v, cx|v.close(window, cx)); }
        this.runtime = None;
        this.model.space_mut("s").unwrap().machine = None;
        this.documents.entry("".into()).or_default().available = true;
        this.person.browser_url = "https://homepage.test/".into();
        let run = |this: &mut super::AppWindow, name: &str, args| {
            let result = this.run_as(name, args, false, window, cx);
            assert!(result.get("error").is_none(), "{name}: {result}");
            result
        };
        let mut run = run;
        let new = run(this, "browser.tab.new", json!({"pane":"page"}));
        let id = new["tab"].as_str().unwrap().to_owned();
        let saved = this.model.tab_of("page").unwrap().root.find("page").unwrap().content().unwrap();
        let super::Content::Browser { url, tabs } = saved else { panic!() };
        assert_eq!(url, "https://homepage.test/");
        assert_eq!(tabs.items.len(), 2);
        assert_eq!(tabs.items[0].url, "https://example.com");
        let initial = tabs.items[0].id.clone();
        run(this, "browser.tab.move", json!({"pane":"page","tab":id,"index":0}));
        run(this, "browser.tab.select", json!({"pane":"page","tab":initial}));
        run(this, "browser.tab.close", json!({"pane":"page","tab":id}));
        assert!(this.model.tab_of("term").is_some());
        run(this, "browser.tab.close", json!({"pane":"page"}));
        assert!(this.model.tab_of("page").is_none());
        assert!(this.model.tab_of("term").is_some());
    });
}

#[test]
fn restored_browser_tabs_are_manageable_without_native_pages() {
    in_own_process("browser_tabs_restored_commands");
}

#[gpui::test]
fn browser_tabs_restored_commands(cx: &mut TestAppContext) {
    if !running_alone("browser_tabs_restored_commands") { return; }
    let (view, cx) = review_browser_window(cx);
    view.update_in(cx, |this, window, cx| {
        #[cfg(all(feature = "browser", target_os = "linux"))]
        {
            for (_, slot) in this.browsers.drain() { slot.view.update(cx, |v, cx|v.close(window, cx)); }
            this.runtime = None;
        }
        this.model.space_mut("s").unwrap().machine = None;
        this.documents.entry("".into()).or_default().available = true;
        let tab = this.model.tab_of("page").unwrap().id.clone();
        let node = this.model.tab_mut(&tab).unwrap().root.find_mut("page").unwrap();
        let super::Kind::Block { content: super::Content::Browser { url, tabs } } = &mut node.kind else { panic!() };
        tabs.ensure(url);
        tabs.insert("restored".into(), "https://restored.test".into());
        *url = tabs.url().unwrap().into();
        this.sync(window, cx);
        for (name, args) in [
            ("browser.tab.select", json!({"pane":"page","tab":"initial"})),
            ("browser.tab.move", json!({"pane":"page","tab":"restored","index":0})),
            ("browser.tab.close", json!({"pane":"page","tab":"restored"})),
        ] {
            let reply = this.run_as(name, args, false, window, cx);
            assert!(reply.get("error").is_none(), "{name}: {reply}");
        }
        let unchanged = this.model.tab_of("page").unwrap().root.clone();
        for args in [json!({"pane":"page","tab":42}), json!({"pane":"page","tab":"missing"})] {
            let reply = this.run_as("browser.tab.select", args, false, window, cx);
            assert!(reply.get("error").is_some(), "{reply}");
            assert_eq!(this.model.tab_of("page").unwrap().root, unchanged);
        }
        let reply = this.run_as("browser.tab.close", json!({"pane":"page"}), false, window, cx);
        assert!(reply.get("error").is_none(), "{reply}");
        assert!(this.model.tab_of("page").is_none());
        assert!(this.model.tab_of("term").is_some());
    });
}

#[test]
fn final_browser_workspace_closes_transactionally_without_terminal_confirmation_bypass() {
    in_own_process("browser_final_workspace_close");
}

#[gpui::test]
fn browser_final_workspace_close(cx: &mut TestAppContext) {
    if !running_alone("browser_final_workspace_close") { return; }
    let (view, cx) = review_browser_window(cx);
    view.update_in(cx, |this, window, cx| {
        #[cfg(all(feature = "browser", target_os = "linux"))]
        {
            for (_, slot) in this.browsers.drain() { slot.view.update(cx, |v, cx|v.close(window, cx)); }
            this.runtime = None;
        }
        this.model = super::Model::default();
        let mut tabs = crate::browser_tabs::BrowserTabs::default();
        tabs.ensure("https://sole.test");
        this.model.add_space("sole", "Browser only", "", super::Tab::with_pane("sole-tab", "Browser", super::Node::block("sole-pane", super::Content::Browser { url:"https://sole.test".into(), tabs })));
        this.documents.entry("".into()).or_default().available = true;
        // A fixed layout and an unavailable document must fail without ever
        // publishing an empty durable tab list or silently regenerating IDs.
        this.model.tab_mut("sole-tab").unwrap().root.find_mut("sole-pane").unwrap().closable = Some(false);
        let fixed = this.model.tab("sole-tab").unwrap().root.clone();
        let reply = this.run_as("browser.tab.close", json!({"pane":"sole-pane"}), false, window, cx);
        assert!(reply.get("error").is_some(), "{reply}");
        assert_eq!(this.model.tab("sole-tab").unwrap().root, fixed);
        this.model.tab_mut("sole-tab").unwrap().root.find_mut("sole-pane").unwrap().closable = Some(true);
        this.documents.get_mut("").unwrap().available = false;
        let protected = this.model.tab("sole-tab").unwrap().root.clone();
        let reply = this.run_as("browser.tab.close", json!({"pane":"sole-pane"}), false, window, cx);
        assert!(reply.get("error").is_some(), "{reply}");
        assert_eq!(this.model.tab("sole-tab").unwrap().root, protected);
        this.documents.get_mut("").unwrap().available = true;
        let reply = this.run_as("browser.tab.close", json!({"pane":"sole-pane"}), false, window, cx);
        assert!(reply.get("error").is_none(), "{reply}");
        assert!(this.model.spaces.is_empty(), "sole browser workspace must really close");

        // Other spaces' terminals must not be confirmed/killed as a side effect.
        this.model.add_space("other", "Unrelated", "", super::Tab::with_pane("other-tab", "Terminal", super::Node::block("other-pane", super::Content::Terminal { terminal:"keep-me".into() })));
        this.model.add_space("web", "Browser", "", super::Tab::with_pane("web-tab", "Browser", super::Node::block("web-pane", super::Content::Browser { url:"https://sole.test".into(), tabs:Default::default() })));
        let closing = this.closing_terminals.clone();
        let reply = this.run_as("browser.tab.close", json!({"pane":"web-pane"}), false, window, cx);
        assert!(reply.get("error").is_none(), "{reply}");
        assert!(this.model.space("web").is_none());
        assert!(this.model.tab_of("other-pane").is_some());
        assert_eq!(this.closing_terminals, closing);
        let reply = this.run_as("space.close", json!({"id":"other"}), false, window, cx);
        assert!(reply.get("error").is_some(), "terminal close confirmation must remain required: {reply}");
        assert!(this.model.tab_of("other-pane").is_some());
    });
}

#[cfg(all(feature = "browser", target_os = "linux"))]
#[test]
fn queued_tab_selection_and_page_commands_reconcile_before_render() {
    in_own_process("browser_selected_owner_before_render");
}

#[cfg(all(feature = "browser", target_os = "linux"))]
#[gpui::test]
fn browser_selected_owner_before_render(cx: &mut TestAppContext) {
    if !running_alone("browser_selected_owner_before_render") { return; }
    let (view, cx) = review_browser_window(cx);
    view.update_in(cx, |this, window, cx| {
        for (_, slot) in this.browsers.drain() { slot.view.update(cx, |v, cx|v.close(window, cx)); }
        this.runtime = None;
        this.model.space_mut("s").unwrap().machine = None;
        this.documents.entry("".into()).or_default().available = true;
        let mut tabs = crate::browser_tabs::BrowserTabs::default();
        tabs.ensure("https://a.test");
        tabs.insert("b".into(), "https://b.test".into());
        tabs.select("initial");
        let super::Kind::Block { content } = &mut this.model.tab_mut("tab").unwrap().root.find_mut("page").unwrap().kind else { panic!() };
        *content = super::Content::Browser { url:"https://a.test".into(), tabs:tabs.clone() };
        // Construct the native owner but never yield to its CEF creation timer:
        // all queued actions and assertions run in this single window update.
        let runtime = gpui_shell::ShellRuntime::new_isolated_with_components(crate::browser_chrome::components()).unwrap();
        this.runtime = Some(runtime);
        this.prepare_browsers(window, cx);
        let native = this.browsers["page"].view.clone();
        this.runtime = None;
        let reply = this.run_as("browser.tab.select", json!({"pane":"page","tab":"b"}), false, window, cx);
        assert!(reply.get("error").is_none(), "{reply}");
        assert_eq!(native.read(cx).current_url(), "https://b.test");
        for command in ["browser.back", "browser.forward", "browser.reload", "browser.stop"] {
            let reply = this.run_as(command, json!({"pane":"page"}), false, window, cx);
            assert!(reply.get("error").is_none(), "{command}: {reply}");
            assert_eq!(native.read(cx).current_url(), "https://b.test");
        }
        let reply = this.run_as("browser.navigate", json!({"pane":"page","url":"https://c.test"}), false, window, cx);
        assert!(reply.get("error").is_none(), "{reply}");
        assert_eq!(native.read(cx).current_url(), "https://c.test");
        let state = crate::browser_chrome::state("page");
        assert_eq!(state["active_tab"], "b");
        assert_eq!(state["tabs"][0]["url"], "https://a.test");
        assert_eq!(state["tabs"][1]["url"], "https://c.test");
        // A new selected tab immediately takes ownership too; no interim page
        // creation, and no accidental load/history mutation of A or B.
        let new = this.run_as("browser.tab.new", json!({"pane":"page","url":"https://new.test"}), false, window, cx);
        assert!(new.get("error").is_none(), "{new}");
        let reply = this.run_as("browser.navigate", json!({"pane":"page","url":"https://new-c.test"}), false, window, cx);
        assert!(reply.get("error").is_none(), "{reply}");
        assert_eq!(native.read(cx).current_url(), "https://new-c.test");
        let state = crate::browser_chrome::state("page");
        assert_eq!(state["active_tab"], new["tab"]);
        assert_eq!(state["tabs"][0]["url"], "https://a.test");
        assert_eq!(state["tabs"][1]["url"], "https://c.test");
        assert!(state["tabs"].as_array().unwrap().iter().all(|t|t["loaded"] == false));
        // Explicit teardown before any task can initialize CEF in fake GPUI.
        this.browsers.remove("page");
        native.update(cx, |native, cx|native.close(window, cx));
    });
}

#[cfg(not(all(feature = "browser", target_os = "linux")))]
#[test]
fn browser_menu_and_edit_commands_fail_closed_without_native_feature() {
    in_own_process("browser_context_default_off");
}

#[cfg(not(all(feature = "browser", target_os = "linux")))]
#[gpui::test]
fn browser_context_default_off(cx: &mut TestAppContext) {
    if !running_alone("browser_context_default_off") { return; }
    let (view, cx) = review_browser_window(cx);
    view.update_in(cx, |this, window, cx| {
        for name in ["browser.copy", "browser.cut", "browser.paste", "browser.address_context", "browser.context_action"] {
            let args = match name {
                "browser.address_context" => json!({"pane":"page", "owner":"7"}),
                "browser.context_action" => json!({"pane":"page", "token":"1", "action":"open_link_new_tab"}),
                _ => json!({"pane":"page"}),
            };
            let reply = this.run_as(name, args, false, window, cx);
            assert!(reply["error"].as_str().unwrap().contains("Browser unavailable"), "{name}: {reply}");
        }
        let state = crate::browser_chrome::state("page");
        assert_eq!(state["available"], false);
        assert!(state["context"].is_null());
    });
}
