//! Local address editing capabilities; never route these actions to Chrome.
use super::*;
use std::sync::atomic::AtomicU64;

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

pub(super) struct Menu {
    token: String,
    focus: FocusHandle,
    copy: u64,
    epoch: u64,
    selection: bool,
    until: Instant,
}

#[cfg(test)]
mod tests {
    use crate::browser::panel::Panel;
    use gpui::{prelude::*, ClipboardEntry, ClipboardItem, Context, Entity, ExternalPaths, IntoElement, Render, TestAppContext, Window};
    use gpui_base::input::{Input, InputState};
    use serde_json::json;

    struct Editor(Entity<InputState>);
    impl Render for Editor {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::div().w_full().child(Input::new(&self.0))
        }
    }

    #[gpui::test]
    fn local_address_owner_rejects_foreign_consumed_hidden_and_oversized_paste(cx: &mut TestAppContext) {
        cx.update(gpui_shell::init);
        let mut editor = None;
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| InputState::new(window,cx).default_value("address selection"));
            editor = Some(input.clone());
            let content = cx.new(|_| Editor(input)).into();
            let root = cx.new(|cx| gpui_shell::ShellRoot::new(content,window,cx));
            let runtime = gpui_shell::ShellRuntime::new_isolated().unwrap();
            let mut panel = Panel::new("address-owner-fixture".into(),"remote-no-core".into(),window,cx);
            panel.set_chrome("address-owner-pane".into(),Some(root),1,runtime,cx);
            window.activate_window();
            panel
        });
        let editor = editor.unwrap();
        cx.run_until_parked();
        cx.update(|window, app| editor.update(app, |input,cx| {
            input.focus(window,cx);
            input.select_all(window,cx);
        }));
        cx.run_until_parked();
        cx.update(|window, app| {
            app.write_to_clipboard(ClipboardItem::new_string("untouched".into()));
            panel.update(app, |panel,cx| {
                let open = panel.dispatch_command("browser.address_context",&json!({"selection":true}),window,cx).unwrap();
                assert_eq!(crate::browser::chrome::state("address-owner-pane")["context_menu"]["source"],"address");
                assert!(panel.dispatch_command("browser.context_action",&json!({"token":"foreign","action":"cut"}),window,cx).is_err());
                assert!(panel.dispatch_command("browser.context_action",&json!({"token":open["token"],"action":"cut"}),window,cx).is_err());
                let open = panel.dispatch_command("browser.address_context",&json!({"selection":true}),window,cx).unwrap();
                panel.visible(false);
                assert!(panel.dispatch_command("browser.context_action",&json!({"token":open["token"],"action":"cut"}),window,cx).is_err());
                panel.visible(true);
            });
            assert_eq!(app.read_from_clipboard().unwrap().text().as_deref(),Some("untouched"));
            assert_eq!(editor.read(app).value().as_str(),"address selection");
            app.write_to_clipboard(ClipboardItem::new_string("x".repeat(65_537)));
            panel.update(app, |panel,cx| {
                let open = panel.dispatch_command("browser.address_context",&json!({"selection":true}),window,cx).unwrap();
                assert!(panel.dispatch_command("browser.context_action",&json!({"token":open["token"],"action":"paste"}),window,cx).is_err());
            });
            assert_eq!(editor.read(app).value().as_str(),"address selection");
            assert_eq!(app.read_from_clipboard().unwrap().text().unwrap().len(),65_537);
            panel.update(app, |panel,cx| {
                assert!(panel.dispatch_command("browser.paste",&json!({}),window,cx).is_err());
            });
            assert_eq!(editor.read(app).value().as_str(),"address selection");
            let mut paths = ExternalPaths::default();
            paths.0.push("/owned/not-a-plaintext-payload".into());
            app.write_to_clipboard(ClipboardItem::from(ClipboardEntry::ExternalPaths(paths)));
            panel.update(app, |panel,cx| {
                assert!(panel.dispatch_command("browser.paste",&json!({}),window,cx).is_err());
            });
            assert_eq!(editor.read(app).value().as_str(),"address selection");
            assert!(matches!(app.read_from_clipboard().unwrap().entries(),[ClipboardEntry::ExternalPaths(_)]));
            panel.update(app, |panel,cx| {
                panel.dispatch_command("browser.address_context",&json!({"selection":true}),window,cx).unwrap();
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(crate::browser::chrome::state("address-owner-pane")["context_menu"].is_null());
    }

    #[gpui::test]
    fn address_menu_requires_a_native_input_and_never_falls_back_to_page(cx: &mut TestAppContext) {
        let (panel, cx) = cx.add_window_view(|window, cx|
            Panel::new("address-fixture".into(), "remote-no-core".into(), window, cx));
        cx.update(|window, app| {
            app.write_to_clipboard(ClipboardItem::new_string("untouched".into()));
            panel.update(app, |panel, cx| {
                panel.focus_handle().focus(window, cx);
                for visible in [true, false] {
                    panel.visible(visible);
                    assert!(panel.dispatch_command("browser.address_context", &json!({"selection":true}), window, cx).is_err());
                    for action in ["copy","cut","paste","select_all"] {
                        assert!(panel.dispatch_command("browser.context_action", &json!({
                            "token":"ad000000000000000000000000000001","action":action}), window, cx).is_err());
                    }
                }
                assert!(panel.dispatch_command("browser.context_dismiss", &json!({}), window, cx).is_ok());
            });
            assert_eq!(app.read_from_clipboard().unwrap().text().as_deref(), Some("untouched"));
        });
    }
}

impl Menu {
    fn current(&self, panel: &Panel, state: &Snapshot) -> bool {
        panel.visible.load(Ordering::Relaxed) && panel.chrome.is_some() && self.copy != 0
            && panel.chrome_copy == self.copy && state.source_epoch == self.epoch
            && Instant::now() < self.until
    }

    fn focused(&self, panel: &Panel, window: &Window, cx: &App) -> bool {
        self.current(panel, &panel.shared.lock().unwrap()) && panel.address_input_focused(window, cx)
            && self.focus.is_focused(window)
    }

    pub(super) fn public_state(&self, panel: &Panel, state: &Snapshot) -> Option<Value> {
        self.current(panel, state).then(|| json!({"source":"address","token":self.token,"target":"",
            "x":0,"y":0,"link":null,"editable":true,"password":false,"read_only":false,"disabled":false,
            "selection_present":self.selection,"can_copy":self.selection,"can_cut":self.selection,"can_paste":true}))
    }
}

impl Panel {
    fn address_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.visible.load(Ordering::Relaxed) && window.is_window_active()
            && self.chrome.is_some() && self.chrome_copy != 0
            && !self.focus.is_focused(window) && self.focus.contains_focused(window, cx)
            && !self.confirm_stop && !self.confirm_takeover
            && window.focused(cx).is_some_and(|focus|
                window.is_action_available_in(&gpui_base::input::Copy,&focus)
                && window.is_action_available_in(&gpui_base::input::Cut,&focus)
                && window.is_action_available_in(&gpui_base::input::Paste,&focus))
    }

    pub(super) fn address_context_command(&mut self, args: &Value, window: &mut Window, cx: &mut Context<Self>) -> std::result::Result<Value,String> {
        self.context_menu = None;
        self.context_pressed = None;
        // Abandon every older page export before capturing this local owner.
        self.cancel_clipboard();
        let result = (|| {
            let selection = args.get("selection").and_then(Value::as_bool).ok_or("Address selection must be a boolean")?;
            if !self.address_input_focused(window, cx) {
                return Err("Address context requires the exact focused native input".to_owned());
            }
            let focus = window.focused(cx).ok_or("Address input lost focus")?;
            let menu = Menu { token:format!("ad00000000000000{:016x}",NEXT_TOKEN.fetch_add(1,Ordering::Relaxed)),
                focus, copy:self.chrome_copy, epoch:self.shared.lock().unwrap().source_epoch,
                selection, until:Instant::now()+Duration::from_secs(10) };
            let token = menu.token.clone();
            self.context_menu = Some(context_menu::Menu::Address(menu));
            Ok(json!({"token":token,"source":"address"}))
        })();
        self.context_error(if result.is_ok() {""} else {"Address context unavailable; no page fallback was used"});
        self.refresh_chrome(cx);
        cx.notify();
        result
    }

    pub(super) fn complete_address(&mut self, window: &Window, cx: &App) {
        if matches!(&self.context_menu, Some(context_menu::Menu::Address(menu)) if !menu.focused(self,window,cx)) {
            self.context_menu = None;
            self.shared.lock().unwrap().revision += 1;
        }
    }

    /// A local menu is consumed even on rejection. It can never fall through to
    /// a core token or a newly focused editor.
    pub(super) fn address_context_action(&mut self, args: &Value, window: &mut Window, cx: &mut Context<Self>) -> std::result::Result<Value,String> {
        let Some(context_menu::Menu::Address(menu)) = self.context_menu.take() else {
            return Err("Address menu is unavailable".into());
        };
        if args["token"].as_str() != Some(menu.token.as_str()) || !menu.focused(self,window,cx) {
            return Err("Address menu owner/token is stale; no action was dispatched".into());
        }
        if args.get("text").is_some() { return Err("Address actions do not accept caller text".into()); }
        let action: Box<dyn Action> = match args["action"].as_str() {
            Some("copy") => Box::new(gpui_base::input::Copy),
            Some("cut") => Box::new(gpui_base::input::Cut),
            Some("paste") => {
                clipboard::paste_text(&cx.read_from_clipboard().ok_or("Clipboard has no plaintext entry")?)
                    .map_err(str::to_owned)?;
                Box::new(gpui_base::input::Paste)
            }
            Some("select_all") => Box::new(gpui_base::input::SelectAll),
            _ => return Err("Unsupported address context action".into()),
        };
        if !menu.focused(self,window,cx) || !window.is_action_available_in(action.as_ref(),&menu.focus) {
            return Err("Address input changed before dispatch; no action was retargeted".into());
        }
        self.cancel_clipboard();
        menu.focus.dispatch_action(action.as_ref(),window,cx);
        Ok(json!({"dispatched":true,"scope":"input"}))
    }
}
