//! Native plaintext boundary. Payloads never enter chrome state, diagnostics or history.
use super::*;
use std::sync::atomic::AtomicU64;

pub const MAX_TEXT_BYTES: usize = 65_536;

#[cfg(test)]
mod tests {
    use super::{MAX_TEXT_BYTES, Outcome, Panel, parse_reply, paste_text, rejected_before_dispatch, shortcut};
    use gpui::{ClipboardEntry, ClipboardItem, ExternalPaths, Modifiers, TestAppContext};
    use serde_json::json;

    #[test]
    fn only_proven_pre_dispatch_rejections_release_without_poisoning() {
        assert!(rejected_before_dispatch(Some("invalid_params")));
        for kind in [None, Some("browser_failed"), Some("not_owner"), Some("stale_generation"), Some("io_error")] {
            assert!(!rejected_before_dispatch(kind));
        }
    }

    #[test]
    fn seven_aliases_require_exact_modifiers() {
        let control = Modifiers { control: true, ..Modifiers::default() };
        let shift = Modifiers { shift: true, ..Modifiers::default() };
        for (key, modifiers, command) in [
            ("c", control, "browser.copy"), ("x", control, "browser.cut"),
            ("v", control, "browser.paste"),
            ("v", Modifiers { shift: true, ..control }, "browser.paste"),
            ("insert", control, "browser.copy"), ("insert", shift, "browser.paste"),
            ("delete", shift, "browser.cut"),
        ] {
            assert_eq!(shortcut(key, &modifiers), Some(command));
            assert_eq!(shortcut(key, &Modifiers { alt: true, ..modifiers }), None);
            assert_eq!(shortcut(key, &Modifiers { platform: true, ..modifiers }), None);
            assert_eq!(shortcut(key, &Modifiers { function: true, ..modifiers }), None);
        }
        assert_eq!(shortcut("c", &Modifiers { shift: true, ..control }), None);
        assert_eq!(shortcut("insert", &Modifiers { shift: true, ..control }), None);
        assert_eq!(shortcut("delete", &control), None);
        assert_eq!(shortcut("v", &Modifiers::default()), None);
    }

    #[gpui::test]
    fn unavailable_page_commands_preserve_os_clipboard(cx: &mut TestAppContext) {
        let (panel, cx) = cx.add_window_view(|window, cx|
            Panel::new("clipboard-fixture".into(), "remote-no-core".into(), window, cx));
        cx.update(|window, app| {
            app.write_to_clipboard(ClipboardItem::new_string("untouched".into()));
            panel.update(app, |panel, cx| {
                panel.focus_handle().focus(window, cx);
                let status = panel.dispatch_command("browser.status", &json!({}), window, cx).unwrap();
                assert!(status.get("error").is_none(), "data must not masquerade as a CLI error reply");
                assert!(status.get("panel_error").is_some());
                for command in ["browser.copy", "browser.cut", "browser.paste"] {
                    assert!(panel.dispatch_command(command, &json!({}), window, cx).is_err());
                }
            });
            assert_eq!(paste_text(&app.read_from_clipboard().unwrap()).unwrap(), "untouched");
        });
    }

    #[test]
    fn copied_payload_is_bounded_and_wrong_operation_results_fail_closed() {
        assert!(matches!(parse_reply(json!({"kind":"copied","text":""}), "copy"), Outcome::Copied(text) if text.is_empty()));
        assert!(matches!(parse_reply(json!({"kind":"copied","text":"secret"}), "paste"), Outcome::Failed));
        assert!(matches!(parse_reply(json!({"kind":"copied","text":"🦀".repeat(16_385)}), "cut"), Outcome::Failed));
        assert!(matches!(parse_reply(json!({"kind":"no-update-observed"}), "cut"), Outcome::NoUpdate));
        assert!(matches!(parse_reply(json!({"kind":"failed","message":"untrusted secret"}), "copy"), Outcome::Failed));
        assert!(matches!(parse_reply(json!({"kind":"copied","text":"x","extra":"data"}), "copy"), Outcome::Failed));
        assert!(matches!(parse_reply(json!({"kind":"link","text":"https://example.invalid/"}), "copy_link"), Outcome::Copied(_)));
        assert!(matches!(parse_reply(json!({"kind":"link","text":"https://example.invalid/"}), "copy"), Outcome::Failed));
        assert!(matches!(parse_reply(json!({"kind":"copied","text":"source"}), "copy_link"), Outcome::Failed));
        assert!(matches!(parse_reply(json!({"kind":"link","text":"x".repeat(16_385)}), "copy_link"), Outcome::Failed));
        assert!(matches!(parse_reply(json!({"kind":"link","text":"url\nsecret"}), "copy_link"), Outcome::Failed));
    }

    #[test]
    fn plaintext_projection_preserves_empty_and_rejects_oversized_utf8() {
        assert_eq!(paste_text(&ClipboardItem::new_string(String::new())), Ok(String::new()));
        assert!(paste_text(&ClipboardItem::new_string("🦀".repeat(16_385))).is_err());
        assert_eq!(paste_text(&ClipboardItem::new_string("🦀".repeat(16_384))).unwrap().len(), MAX_TEXT_BYTES);
        assert!(paste_text(&ClipboardItem::from(ClipboardEntry::ExternalPaths(ExternalPaths::default()))).is_err());
    }
}

/// Only actual plaintext entries are accepted; paths are never coerced to text.
pub fn paste_text(item: &ClipboardItem) -> std::result::Result<String, &'static str> {
    let [ClipboardEntry::String(text)] = item.entries() else {
        return Err("Clipboard must contain one plaintext entry");
    };
    if text.text.len() > MAX_TEXT_BYTES {
        return Err("Clipboard plaintext exceeds 64KiB UTF-8");
    }
    Ok(text.text.clone())
}

// No Debug on payload-bearing types.
pub enum Outcome {
    Copied(String),
    NoUpdate,
    Pasted,
    Failed,
    Rejected,
    Abandoned,
}

pub fn parse_reply(reply: Value, operation: &str) -> Outcome {
    let Some(object) = reply.as_object() else { return Outcome::Failed };
    match reply["kind"].as_str() {
        Some("copied") if matches!(operation,"copy"|"cut") && object.len() == 2 => {
            match reply["text"].as_str().filter(|text| text.len() <= MAX_TEXT_BYTES) {
                Some(text) => Outcome::Copied(text.to_owned()),
                None => Outcome::Failed,
            }
        }
        Some("link") if operation == "copy_link" && object.len() == 2 => {
            match reply["text"].as_str().filter(|text| super::context_menu::valid_link(text)) {
                Some(text) => Outcome::Copied(text.to_owned()),
                None => Outcome::Failed,
            }
        }
        Some("no-update-observed") if matches!(operation,"copy"|"cut") && object.len() == 1 => Outcome::NoUpdate,
        Some("paste-dispatched") if operation == "paste" && object.len() == 1 => Outcome::Pasted,
        // The remote message is deliberately never copied into UI state/logs.
        _ => Outcome::Failed,
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static BUSY: AtomicU64 = AtomicU64::new(0);
static EXPORT_EPOCH: AtomicU64 = AtomicU64::new(0);

struct Lease(u64);
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = BUSY.compare_exchange(self.0, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

#[derive(Clone)]
pub(super) struct Owner {
    group: String,
    generation: String,
    target: String,
    epoch: u64,
    export_epoch: u64,
}
impl Owner {
    fn current(&self, state: &Snapshot) -> bool {
        state.ready && state.group == self.group && state.generation == self.generation && state.selected == self.target
            && state.source_epoch == self.epoch
            && EXPORT_EPOCH.load(Ordering::SeqCst) == self.export_epoch
    }
}

pub(super) struct Request {
    id: u64,
    owner: Owner,
    operation: &'static str,
    _lease: Lease,
}
pub(super) struct Completion {
    request: Request,
    outcome: Outcome,
}
pub(super) struct Pending {
    id: u64,
    operation: &'static str,
    owner: Owner,
}

pub(super) fn execute(core: &Core, state: &Arc<Mutex<Snapshot>>, request: Request, method: &str, params: Value) {
    let outcome = if !request.owner.current(&state.lock().unwrap()) {
        Outcome::Abandoned
    } else {
        match core.call(method, params) {
            Ok(reply) => parse_reply(reply, request.operation),
            Err(error) if rejected_before_dispatch(crate::hc::error_kind(&error)) => Outcome::Rejected,
            Err(_) => {
                // An RPC timeout is not proof that native editing drained. Fail
                // closed process-wide instead of permitting overlapping retries.
                BUSY.store(u64::MAX, Ordering::SeqCst);
                Outcome::Failed
            }
        }
    };
    let mut state = state.lock().unwrap();
    state.clipboard_completion = Some(Completion { request, outcome });
    state.revision += 1;
}

// The clipboard endpoint validates this class before any source edit. Do not
// generalize to lease errors: those may also arrive after native dispatch.
fn rejected_before_dispatch(kind: Option<&str>) -> bool {
    kind == Some("invalid_params")
}

/// Exact native aliases, after app key bindings have had first refusal.
pub fn shortcut(key: &str, modifiers: &Modifiers) -> Option<&'static str> {
    if modifiers.alt || modifiers.platform || modifiers.function { return None; }
    match (key, modifiers.control, modifiers.shift) {
        ("c" | "insert", true, false) => Some("browser.copy"),
        ("x", true, false) | ("delete", false, true) => Some("browser.cut"),
        ("v", true, _) | ("insert", false, true) => Some("browser.paste"),
        _ => None,
    }
}

impl Panel {
    /// Call before any immediate native OS write (including Copy Link). This
    /// abandons exports across panels, but never releases an in-flight lease.
    pub fn cancel_clipboard(&self) {
        EXPORT_EPOCH.fetch_add(1, Ordering::SeqCst);
        self.invalidate_clipboard();
    }

    pub(super) fn invalidate_clipboard(&self) {
        let mut state = self.shared.lock().unwrap();
        state.source_epoch += 1;
        state.revision += 1;
    }

    pub(super) fn clipboard_command(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) -> std::result::Result<Value, String> {
        let result = self.queue_clipboard(name, window, cx);
        if let Err(error) = &result {
            let mut state = self.shared.lock().unwrap();
            state.clipboard_error = error.clone();
            state.clipboard_outcome = "rejected";
            state.revision += 1;
        }
        self.refresh_chrome(cx);
        cx.notify();
        result
    }

    fn queue_clipboard(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) -> std::result::Result<Value, String> {
        let operation = match name {
            "browser.copy" => "copy", "browser.cut" => "cut", "browser.paste" => "paste",
            _ => return Err("Unsupported clipboard operation".into()),
        };
        if !self.visible.load(Ordering::Relaxed) || !window.is_window_active() {
            return Err("Browser clipboard requires a visible, active window".into());
        }
        if !self.focus.is_focused(window) {
            // Dispatch synchronously on the captured editor handle, never on a
            // page root or whatever happens to gain focus on a later tick.
            if self.chrome.is_some() && self.focus.contains_focused(window, cx) {
                let action: Box<dyn Action> = match operation {
                    "copy" => Box::new(gpui_base::input::Copy),
                    "cut" => Box::new(gpui_base::input::Cut),
                    _ => Box::new(gpui_base::input::Paste),
                };
                if let Some(focus) = window.focused(cx)
                    && window.is_action_available_in(action.as_ref(), &focus) {
                    if operation == "paste" {
                        paste_text(&cx.read_from_clipboard().ok_or("Clipboard has no plaintext entry")?)
                            .map_err(str::to_owned)?;
                    }
                    self.cancel_clipboard();
                    focus.dispatch_action(action.as_ref(), window, cx);
                    return Ok(json!({"dispatched":true,"scope":"input"}));
                }
            }
            return Err("Browser clipboard requires the exact focused page or native input".into());
        }
        if self.editing_address { return Err("Focus the native address input to edit its clipboard".into()); }
        if self.confirm_stop || self.confirm_takeover { return Err("Close the browser confirmation before editing".into()); }
        if !self.machine.is_empty() { return Err("Remote browser clipboard unavailable; local fallback is forbidden".into()); }
        if self.tools_focus { return Err("DevTools clipboard scope is not supported; page clipboard was not used".into()); }
        self.invalidate_clipboard();
        self.enqueue_clipboard(operation, None, cx)
    }

    pub(super) fn clipboard_context_command(&mut self, operation: &'static str, token: &str, owner: &super::context_menu::Owner, window: &mut Window, cx: &mut Context<Self>) -> std::result::Result<Value,String> {
        let result = if !self.visible.load(Ordering::Relaxed) || !window.is_window_active()
            || !self.focus.is_focused(window) || self.tools_focus || self.editing_address
            || self.confirm_stop || self.confirm_takeover || !self.machine.is_empty() {
            Err("Native context clipboard requires the exact visible, active page focus".into())
        } else {
            self.enqueue_clipboard(operation, Some((token,owner)), cx)
        };
        if let Err(error) = &result {
            let mut state = self.shared.lock().unwrap();
            state.clipboard_error = error.clone();
            state.clipboard_outcome = "rejected";
            state.revision += 1;
        }
        result
    }

    fn enqueue_clipboard(&mut self, operation: &'static str, context: Option<(&str,&super::context_menu::Owner)>, cx: &mut Context<Self>) -> std::result::Result<Value,String> {
        if self.pending_clipboard.is_some() || BUSY.load(Ordering::SeqCst) != 0 {
            return Err("Browser clipboard is busy or blocked after an uncertain completion; no retry was sent".into());
        }
        // Only an explicit Paste reads the OS, and validation precedes enqueue.
        let text = if operation == "paste" {
            Some(paste_text(&cx.read_from_clipboard().ok_or("Clipboard has no plaintext entry")?).map_err(str::to_owned)?)
        } else { None };
        let mut state = self.shared.lock().unwrap();
        if !state.ready || state.selected.is_empty() || !state.owns_target(&state.selected) {
            return Err("Browser controller or selected page is not ready".into());
        }
        if let Some((_,owner)) = context {
            if !owner.current(&state) || !state.context_capable {
                return Err("Native context target/capability is no longer current".into());
            }
        }
        if !state.clipboard_capable { return Err("Core does not support clipboard_text_v1".into()); }
        if state.clipboard_poisoned { return Err("Browser editing failed; stop/restart the browser before retrying".into()); }
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        BUSY.compare_exchange(0, id, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "Browser clipboard is busy".to_owned())?;
        let owner = Owner { group:state.group.clone(), generation: state.generation.clone(), target: state.selected.clone(),
            epoch: state.source_epoch, export_epoch: EXPORT_EPOCH.load(Ordering::SeqCst) };
        let mut params = json!({"workspace":self.workspace,"generation":owner.generation,"target":owner.target});
        if !owner.group.is_empty() { params["group"] = json!(owner.group); }
        let method = if let Some((token,_)) = context {
            params["token"] = token.into();
            params["action"] = operation.into();
            "browser.context_action"
        } else {
            params["operation"] = operation.into();
            "browser.clipboard"
        };
        if let Some(text) = text { params["text"] = text.into(); }
        let request = Request { id, owner: owner.clone(), operation, _lease: Lease(id) };
        self.commands.try_send(Command { method, params, follow: false, clipboard: Some(request), context: None })
            .map_err(|_| "Browser clipboard queue full/disconnected; nothing was dispatched".to_owned())?;
        self.pending_clipboard = Some(Pending { id, operation, owner });
        state.clipboard_error.clear();
        state.clipboard_outcome = "queued";
        state.revision += 1;
        Ok(json!({"queued":true,"request_id":id}))
    }

    pub(super) fn complete_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let completion = self.shared.lock().unwrap().clipboard_completion.take();
        let Some(completion) = completion else {
            if let Some(pending) = &self.pending_clipboard
                && BUSY.load(Ordering::SeqCst) != pending.id {
                self.pending_clipboard = None;
                let mut state = self.shared.lock().unwrap();
                state.clipboard_outcome = "abandoned";
                state.clipboard_error = "Clipboard queue ended; editing may have occurred. Do not automatically retry Cut.".into();
                state.revision += 1;
            }
            return;
        };
        let mut state = self.shared.lock().unwrap();
        let valid = self.pending_clipboard.as_ref().is_some_and(|pending|
            pending.id == completion.request.id && pending.owner.current(&state))
            && completion.request.owner.current(&state)
            && self.visible.load(Ordering::Relaxed) && window.is_window_active()
            && self.focus.is_focused(window) && !self.editing_address && !self.tools_focus
            && !self.confirm_stop && !self.confirm_takeover;
        self.pending_clipboard = None;
        if matches!(&completion.outcome, Outcome::Failed) {
            state.clipboard_poisoned = true;
            state.clipboard_error = "Native clipboard failed; editing may have occurred. Do not automatically retry Cut. Stop/restart browser; uncertain transport additionally requires app restart.".into();
            state.clipboard_outcome = "failed";
        } else if matches!(&completion.outcome, Outcome::Rejected) {
            state.clipboard_error = "Core rejected the clipboard request before editing; OS clipboard unchanged.".into();
            state.clipboard_outcome = "rejected";
        } else if !valid || matches!(&completion.outcome, Outcome::Abandoned) {
            state.clipboard_outcome = "abandoned";
            state.clipboard_error = "Clipboard owner changed; export discarded. Editing may have occurred; do not automatically retry Cut.".into();
        } else {
            state.clipboard_error.clear();
            state.clipboard_outcome = match &completion.outcome {
                Outcome::Copied(text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    "copied"
                }
                Outcome::NoUpdate => "no-update-observed",
                Outcome::Pasted => "paste-dispatched",
                _ => "abandoned",
            };
        }
        state.revision += 1;
        // Completion owns the global lease until after this UI-thread OS write.
    }

    pub(super) fn clipboard_state(&self, state: &Snapshot) -> Value {
        json!({"clipboard_readiness":if state.clipboard_poisoned || BUSY.load(Ordering::SeqCst) == u64::MAX {"failed"} else if !state.ready {"not-ready"}
            else if !state.clipboard_capable {"unsupported"} else if BUSY.load(Ordering::SeqCst) != 0 {"busy"} else {"ready"},
            "clipboard_error":if state.clipboard_error.is_empty(){None}else{Some(&state.clipboard_error)},
            "clipboard_outcome":state.clipboard_outcome,
            "pending_clipboard":self.pending_clipboard.as_ref().map(|pending|json!({"request_id":pending.id,"operation":pending.operation}))})
    }
}
