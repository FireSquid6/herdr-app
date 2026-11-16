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
        this.model.add_space("s", "Restored remote", "", super::Tab::with_pane("tab", "Browser", super::Node::block("page", super::Content::Browser { url: "https://example.com".into() })));
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
        this.model.add_tab("s", super::Tab::with_pane("hidden", "Hidden", super::Node::block("hidden-page", super::Content::Browser { url: "https://example.org".into() })));
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
