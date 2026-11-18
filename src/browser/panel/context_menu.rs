//! One-shot native menu capabilities. Only validated hit metadata reaches chrome.
use super::*;

const BOOLS: [&str; 8] = ["editable", "password", "read_only", "disabled", "selection_present", "can_copy", "can_cut", "can_paste"];
pub const MAX_LINK_BYTES: usize = 16_384;

#[cfg(test)]
mod tests {
    use super::{parse_metadata, valid_action_reply, valid_link, valid_point};
    use crate::browser::panel::Panel;
    use gpui::{ClipboardItem, TestAppContext};
    use serde_json::json;

    fn reply() -> serde_json::Value {
        json!({"token":"0123456789abcdef0123456789abcdef","target":"page",
            "document":{"frame":"frame","loader":"loader"},"link":"https://example.invalid/",
            "editable":false,"password":false,"read_only":false,"disabled":false,
            "selection_present":true,"can_copy":true,"can_cut":false,"can_paste":false})
    }

    #[gpui::test]
    fn unavailable_or_hidden_context_actions_never_write_the_os_clipboard(cx: &mut TestAppContext) {
        let (panel, cx) = cx.add_window_view(|window, cx|
            Panel::new("context-fixture".into(), "remote-no-core".into(), window, cx));
        cx.update(|window, app| {
            app.write_to_clipboard(ClipboardItem::new_string("untouched".into()));
            panel.update(app, |panel, cx| {
                panel.focus_handle().focus(window, cx);
                for visible in [true, false] {
                    panel.visible(visible);
                    for action in ["copy", "cut", "paste", "copy_link", "open_link", "new_tab", "select_all"] {
                        assert!(panel.dispatch_command("browser.context_action", &json!({
                            "token":"0123456789abcdef0123456789abcdef","action":action}), window, cx).is_err());
                    }
                }
                assert!(panel.dispatch_command("browser.context_dismiss", &json!({}), window, cx).is_ok());
            });
            assert_eq!(app.read_from_clipboard().unwrap().text().as_deref(), Some("untouched"));
        });
    }

    #[test]
    fn only_bounded_points_and_exact_nonclipboard_acknowledgements_are_accepted() {
        assert!(valid_point(0.0, 4095.5));
        for (x,y) in [(f64::NAN,0.0),(0.0,f64::INFINITY),(-1.0,0.0),(4096.0,0.0)] {
            assert!(!valid_point(x,y));
        }
        assert!(valid_action_reply(&json!({"kind":"action-dispatched","action":"select_all"}),"select_all"));
        assert!(valid_action_reply(&json!({"kind":"action-dispatched","action":"new_tab","target":"new","opener_preserved":false}),"new_tab"));
        assert!(valid_action_reply(&json!({"kind":"action-dispatched","action":"new_tab","target":"new","url":"https://owned.test/link","opener_preserved":false}),"new_tab"));
        assert!(!valid_action_reply(&json!({"kind":"action-dispatched","action":"new_tab","target":"new","url":"javascript:alert(1)","opener_preserved":false}),"new_tab"));
        for reply in [json!({"kind":"copied","text":"source plaintext"}),
            json!({"kind":"action-dispatched","action":"open_link"}),
            json!({"kind":"action-dispatched","action":"select_all","text":"source plaintext"})] {
            assert!(!valid_action_reply(&reply,"select_all"));
        }
    }

    #[test]
    fn metadata_boundary_accepts_only_the_documented_projection() {
        assert!(parse_metadata(&reply(), "page").is_ok());
        for (key, value) in [("text", json!("source plaintext")), ("can_copy", json!("true")),
            ("token", json!("not-a-token")), ("link", json!("https://example.invalid/\nsecret")),
            ("link", json!("x".repeat(16_385))), ("target", json!("another-page")),
            ("document", json!({"frame":"f","loader":"l","text":"source plaintext"}))] {
            let mut invalid = reply();
            invalid[key] = value;
            assert!(parse_metadata(&invalid, "page").is_err());
        }
        assert!(valid_link(&"🦀".repeat(4096)));
        assert!(!valid_link(&"🦀".repeat(4097)));
    }
}

pub fn valid_link(text: &str) -> bool {
    text.len() <= MAX_LINK_BYTES && !text.chars().any(char::is_control)
}

fn identity(value: &Value) -> Option<String> {
    value.as_str().filter(|text| !text.is_empty() && text.len() <= 1024
        && !text.chars().any(char::is_control)).map(str::to_owned)
}

pub struct Metadata {
    token: String,
    target: String,
    frame: String,
    loader: String,
    link: Option<String>,
    flags: [bool; 8],
}

/// Public trust boundary; no arbitrary source properties survive this parser.
pub fn parse_metadata(value: &Value, target: &str) -> std::result::Result<Metadata, &'static str> {
    let invalid = "Invalid native context metadata";
    let object = value.as_object().ok_or(invalid)?;
    if object.len() != 12 || value["target"].as_str() != Some(target) { return Err(invalid); }
    let token = value["token"].as_str().filter(|text| text.len() == 32
        && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).ok_or(invalid)?.to_owned();
    let target = identity(&value["target"]).ok_or(invalid)?;
    let document = value["document"].as_object().filter(|object| object.len() == 2).ok_or(invalid)?;
    let frame = document.get("frame").and_then(identity).ok_or(invalid)?;
    let loader = document.get("loader").and_then(identity).ok_or(invalid)?;
    let link = match value.get("link") {
        Some(Value::Null) => None,
        Some(Value::String(text)) if valid_link(text) => Some(text.clone()),
        _ => return Err(invalid),
    };
    let mut flags = [false; 8];
    for (index, name) in BOOLS.iter().enumerate() {
        flags[index] = value[*name].as_bool().ok_or(invalid)?;
    }
    Ok(Metadata { token, target, frame, loader, link, flags })
}

impl Metadata {
    fn public_state(&self, x: f64, y: f64) -> Value {
        let mut value = json!({"source":"page","token":self.token,"target":self.target,
            "document":{"frame":self.frame,"loader":self.loader},"link":self.link,"x":x,"y":y});
        for (index, name) in BOOLS.iter().enumerate() { value[*name] = self.flags[index].into(); }
        value
    }
    fn permits(&self, action: &str) -> bool {
        match action {
            "copy" => self.flags[5], "cut" => self.flags[6], "paste" => self.flags[7],
            "copy_link" | "open_link" | "new_tab" => self.link.is_some(),
            "select_all" => true,
            _ => false,
        }
    }
}

#[derive(Clone)]
pub(super) struct Owner {
    group: String,
    generation: String,
    target: String,
    epoch: u64,
}
impl Owner {
    fn capture(state: &Snapshot) -> Self {
        Self { group:state.group.clone(), generation: state.generation.clone(), target: state.selected.clone(), epoch: state.source_epoch }
    }
    fn scope(&self, mut params: Value) -> Value {
        if !self.group.is_empty() { params["group"] = json!(self.group); }
        params
    }
    pub(super) fn current(&self, state: &Snapshot) -> bool {
        state.ready && state.group == self.group && state.generation == self.generation && state.selected == self.target
            && state.source_epoch == self.epoch
            && state.owns_target(&self.target)
    }
}

pub(super) enum Menu {
    Page(PageMenu),
    Address(super::address_menu::Menu),
}
pub(super) struct PageMenu {
    owner: Owner,
    metadata: Metadata,
    x: f64,
    y: f64,
    until: Instant,
}
pub(super) struct RightPress {
    owner: Owner,
    x: f64,
    y: f64,
}
pub(super) enum Kind {
    Query { x: f64, y: f64 },
    Action(&'static str),
}
pub(super) struct Request {
    owner: Owner,
    kind: Kind,
    until: Instant,
}
enum ResultKind {
    Metadata(Metadata),
    Action(Value),
    Failed,
    Abandoned,
}
pub(super) struct Completion {
    request: Request,
    result: ResultKind,
}

pub(super) fn execute(core: &Core, state: &Arc<Mutex<Snapshot>>, request: Request, params: Value) {
    let result = if Instant::now() >= request.until || !request.owner.current(&state.lock().unwrap()) {
        ResultKind::Abandoned
    } else {
        match &request.kind {
            Kind::Query { .. } => match core.call("browser.context", params) {
                Ok(reply) => parse_metadata(&reply, &request.owner.target)
                    .map(ResultKind::Metadata).unwrap_or(ResultKind::Failed),
                Err(_) => ResultKind::Failed,
            },
            Kind::Action(action) => execute_action(core, state, &request.owner, action, params),
        }
    };
    let mut state = state.lock().unwrap();
    // One slot: an obsolete query/action result is replaced, never accumulated.
    state.context_completion = Some(Completion { request, result });
    state.revision += 1;
}

fn execute_action(core: &Core, state: &Arc<Mutex<Snapshot>>, owner: &Owner, action: &str, params: Value) -> ResultKind {
    let lease = owner.scope(json!({"workspace":params["workspace"],"generation":owner.generation}));
    let Ok(reply) = core.call("browser.context_action", params) else { return ResultKind::Failed };
    if !valid_action_reply(&reply, action) { return ResultKind::Failed; }
    if !owner.current(&state.lock().unwrap()) { return ResultKind::Abandoned; }
    if action == "new_tab" {
        let mut select = lease.clone();
        if owner.group.is_empty() { select["target"] = reply["target"].clone(); }
        else {
            let Ok(tabs) = core.call("browser.tabs", lease.clone()) else { return ResultKind::Failed; };
            let Some(tab) = tabs["tabs"].as_array().and_then(|tabs| tabs.iter().find(|tab| tab["target"] == reply["target"])) else { return ResultKind::Failed; };
            select["tab"] = tab["id"].clone();
        }
        if core.call("browser.tab.select", select).is_err() { return ResultKind::Failed; }
    }
    match core.call("browser.tabs", lease) {
        Ok(tabs) => ResultKind::Action(tabs),
        Err(_) => ResultKind::Failed,
    }
}

/// Only the documented non-clipboard action acknowledgement is accepted.
pub fn valid_action_reply(reply: &Value, action: &str) -> bool {
    let Some(object) = reply.as_object() else { return false };
    if reply["kind"] != "action-dispatched" || reply["action"] != action { return false; }
    match action {
        "new_tab" => (object.len() == 4 && !object.contains_key("url")
            || object.len() == 5 && reply["url"].as_str().is_some_and(|url|
                url != "about:blank" && super::super::saved::safe_url(url)))
            && identity(&reply["target"]).is_some() && reply["opener_preserved"] == false,
        "open_link" | "select_all" => object.len() == 2,
        _ => false,
    }
}

impl Panel {
    fn context_ready(&self, window: &Window) -> bool {
        self.visible.load(Ordering::Relaxed) && window.is_window_active() && self.focus.is_focused(window)
            && !self.tools_focus && !self.editing_address && !self.confirm_stop && !self.confirm_takeover
            && self.machine.is_empty()
    }

    pub(super) fn context_error(&self, message: &'static str) {
        let mut state = self.shared.lock().unwrap();
        state.context_error = message;
        state.revision += 1;
    }

    pub(super) fn context_state(&self, state: &Snapshot) -> Value {
        let menu = match &self.context_menu {
            Some(Menu::Address(menu)) => menu.public_state(self,state),
            Some(Menu::Page(menu)) if menu.owner.current(state) && Instant::now() < menu.until =>
                Some(menu.metadata.public_state(menu.x,menu.y)),
            _ => None,
        };
        let address = menu.as_ref().is_some_and(|menu|menu["source"] == "address");
        json!({"context_menu":menu,
            "context_error":if state.context_error.is_empty(){None}else{Some(state.context_error)},
            "context_readiness":if address {"ready"} else if !state.ready {"not-ready"} else if !state.context_capable {"unsupported"} else {"ready"}})
    }

    pub(super) fn complete_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.complete_address(window,cx);
        let completion = self.shared.lock().unwrap().context_completion.take();
        if self.context_menu.as_ref().is_some_and(|menu| matches!(menu, Menu::Page(menu) if Instant::now() >= menu.until
            || !self.context_ready(window) || !menu.owner.current(&self.shared.lock().unwrap()))) {
            self.context_menu = None;
            // The source epoch already changed, or only this menu expired.
            // Do not abandon a newer clipboard operation while pruning it.
            self.context_error("");
        }
        let Some(completion) = completion else { return };
        if Instant::now() >= completion.request.until || !self.context_ready(window)
            || !completion.request.owner.current(&self.shared.lock().unwrap()) { return; }
        match completion.result {
            ResultKind::Metadata(metadata) => {
                if let Kind::Query { x, y } = completion.request.kind {
                    self.context_menu = Some(Menu::Page(PageMenu { owner: completion.request.owner, metadata, x, y,
                        until: completion.request.until }));
                    self.context_error("");
                }
            }
            ResultKind::Action(tabs) => {
                self.shared.lock().unwrap().tabs(tabs);
                self.context_error("");
            }
            ResultKind::Failed => self.context_error("Native context operation failed; an action may have occurred. Do not automatically retry."),
            ResultKind::Abandoned => {},
        }
    }

    fn queue_context_pointer(&self, event: Value, expected: Option<&Owner>) -> std::result::Result<Owner, &'static str> {
        let mut state = self.shared.lock().unwrap();
        if !state.ready || expected.is_some_and(|owner| !owner.current(&state)) {
            return Err("Native context controller/target changed; no pointer was dispatched");
        }
        if event["action"] == "down" { state.source_epoch += 1; }
        state.revision += 1;
        let params = json!({"surface":{"kind":"page","target":state.selected},"event":event});
        self.commands.try_send(Command { method:"browser.input",params,follow:false,clipboard:None,context:None })
            .map_err(|_| "Native context pointer queue is full/disconnected; menu was not queried")?;
        Ok(Owner::capture(&state))
    }

    pub(super) fn context_pointer_down(&mut self, tools: bool, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.context_menu = None;
        self.context_pressed = None;
        self.cancel_clipboard();
        self.focus.focus(window, cx);
        self.tools_focus = tools;
        self.editing_address = false;
        let point = event.position - self.page_bounds.get().origin;
        let (x, y) = (f64::from(f32::from(point.x)), f64::from(f32::from(point.y)));
        let result = (|| -> std::result::Result<(), &'static str> {
            if tools { return Err("DevTools context menu is unsupported; no page fallback was used"); }
            if !self.context_ready(window) { return Err("Native context menu requires the visible, active page focus"); }
            {
                let state = self.shared.lock().unwrap();
                if !state.ready || !state.context_capable || state.selected.is_empty()
                    || !state.owns_target(&state.selected) {
                    return Err("Core context_menu_v1 or selected page is not ready");
                }
            }
            if !valid_point(x,y) { return Err("Native context point is outside the page viewport"); }
            let owner = self.queue_context_pointer(json!({"type":"pointer","action":"down","x":x,"y":y,"button":"right",
                "buttons":2,"modifiers":mods(&event.modifiers),"click_count":event.click_count}), None)?;
            self.context_pressed = Some(RightPress { owner, x, y });
            Ok(())
        })();
        self.context_error(result.err().unwrap_or(""));
        self.refresh_chrome(cx);
        cx.notify();
        cx.stop_propagation();
    }

    pub(super) fn context_pointer_up(&mut self, tools: bool, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(press) = self.context_pressed.take() else { return };
        if tools || !self.context_ready(window) || !press.owner.current(&self.shared.lock().unwrap()) { return; }
        let point = event.position - self.page_bounds.get().origin;
        let owner = match self.queue_context_pointer(json!({"type":"pointer","action":"up","x":f32::from(point.x),"y":f32::from(point.y),
            "button":"right","buttons":0,"modifiers":mods(&event.modifiers),"click_count":1}), Some(&press.owner)) {
            Ok(owner) => owner,
            Err(error) => {
                self.context_error(error);
                self.refresh_chrome(cx);
                cx.notify();
                cx.stop_propagation();
                return;
            }
        };
        let params = owner.scope(json!({"workspace":self.workspace,"generation":owner.generation,
            "surface":{"kind":"page","target":owner.target},"x":press.x,"y":press.y}));
        let request = Request { owner, kind: Kind::Query { x:press.x, y:press.y }, until:Instant::now()+Duration::from_secs(10) };
        if self.commands.try_send(Command { method:"browser.context",params,follow:false,clipboard:None,context:Some(request) }).is_err() {
            self.context_error("Native context queue is full/disconnected; no query was dispatched");
        }
        cx.notify();
        cx.stop_propagation();
    }

    pub(super) fn context_command(&mut self, name: &str, args: &Value, window: &mut Window, cx: &mut Context<Self>) -> std::result::Result<Value,String> {
        if name == "browser.context_dismiss" {
            self.context_menu = None;
            self.context_pressed = None;
            self.cancel_clipboard();
            self.context_error("");
            self.refresh_chrome(cx);
            cx.notify();
            return Ok(json!({"dismissed":true}));
        }
        let result = if matches!(&self.context_menu, Some(Menu::Address(_))) {
            self.address_context_action(args, window, cx)
        } else {
            self.queue_context_action(args, window, cx)
        };
        if result.is_err() { self.context_error("Native context action rejected; menu is stale, unavailable or busy. No retry was sent."); }
        self.refresh_chrome(cx);
        cx.notify();
        result
    }

    fn queue_context_action(&mut self, args: &Value, window: &mut Window, cx: &mut Context<Self>) -> std::result::Result<Value,String> {
        if args.get("text").is_some() { return Err("Native context actions do not accept caller text".into()); }
        let action = match args["action"].as_str() {
            Some("copy") => "copy", Some("cut") => "cut", Some("paste") => "paste", Some("copy_link") => "copy_link",
            Some("open_link") => "open_link", Some("new_tab") => "new_tab", Some("select_all") => "select_all",
            _ => return Err("Unsupported native context action".into()),
        };
        let Some(Menu::Page(menu)) = self.context_menu.take() else {
            return Err("Native page context menu is unavailable".into());
        };
        if !self.context_ready(window) || Instant::now() >= menu.until
            || !menu.owner.current(&self.shared.lock().unwrap())
            || args["token"].as_str() != Some(menu.metadata.token.as_str()) || !menu.metadata.permits(action) {
            self.invalidate_clipboard();
            return Err("Native context owner/token/action is stale or unavailable".into());
        }
        // Consumed locally before any enqueue, even if a busy queue rejects it.
        self.cancel_clipboard();
        let owner = {
            let state = self.shared.lock().unwrap();
            if !state.ready || state.generation != menu.owner.generation || state.selected != menu.owner.target
                || state.source_epoch != menu.owner.epoch + 1 {
                return Err("Native context owner changed before enqueue; action was not retargeted".into());
            }
            Owner::capture(&state)
        };
        if matches!(action,"copy"|"cut"|"paste"|"copy_link") {
            return self.clipboard_context_command(action, &menu.metadata.token, &owner, window, cx);
        }
        let params = owner.scope(json!({"workspace":self.workspace,"generation":owner.generation,
            "target":owner.target,"token":menu.metadata.token,"action":action}));
        let request = Request { owner, kind:Kind::Action(action), until:menu.until };
        self.commands.try_send(Command { method:"browser.context_action",params,follow:false,clipboard:None,context:Some(request) })
            .map_err(|_|"Native context queue is full/disconnected; no action was dispatched".to_owned())?;
        Ok(json!({"queued":true}))
    }
}

pub fn valid_point(x: f64, y: f64) -> bool {
    x.is_finite() && y.is_finite() && (0.0..4096.0).contains(&x) && (0.0..4096.0).contains(&y)
}
