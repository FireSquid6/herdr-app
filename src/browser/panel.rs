//! Thin native Stage A panel, consuming core-owned browser wire revision 2.
use super::*;

mod address_menu;
pub mod clipboard;
pub mod context_menu;
mod runtime;
mod groups;
use crate::hc::Core;
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    #[default]
    Starting,
    Detached,
    Running,
    Blocked,
    Revoked,
    Stopped,
    Failed,
}

#[derive(Default)]
struct Snapshot {
    phase: Phase,
    generation: String,
    browser_pid: u32,
    instance: String,
    group: String,
    selected_tab: String,
    selected: String,
    tabs: Vec<Value>,
    confirmed_metadata: Value,
    page: Option<Arc<RenderImage>>,
    devtools: Option<Arc<RenderImage>>,
    error: String,
    ready: bool,
    tools_open: bool,
    tools_wanted: bool,
    tools_target: String,
    tools_requested_target: String,
    tools_epoch: u64,
    revision: u64,
    metadata_revision: u64,
    input_pending: u64,
    source_epoch: u64,
    clipboard_capable: bool,
    clipboard_poisoned: bool,
    clipboard_error: String,
    clipboard_outcome: &'static str,
    clipboard_completion: Option<clipboard::Completion>,
    context_capable: bool,
    context_error: &'static str,
    context_completion: Option<context_menu::Completion>,
}
impl Snapshot {
    fn tabs(&mut self, tabs: Value) {
        if !self.group.is_empty() && tabs["group"].as_str() != Some(self.group.as_str()) { return; }
        let revision = tabs["revision"].as_u64().unwrap_or(0);
        if revision < self.metadata_revision {
            return;
        }
        self.metadata_revision = revision;
        self.confirmed_metadata = tabs.clone();
        let selected_tab = tabs["selected"].as_str().unwrap_or_default().to_owned();
        let entries = tabs["tabs"].as_array().cloned().unwrap_or_default();
        let selected = if tabs["group"].is_string() {
            entries.iter().find(|tab| tab["id"] == selected_tab)
                .map(Self::entry_target).unwrap_or_default().to_owned()
        } else { selected_tab.clone() };
        let next_url = entries.iter().find(|tab| tab["id"] == selected_tab)
            .and_then(|tab| tab["url"].as_str()).unwrap_or_default();
        if selected != self.selected || selected_tab != self.selected_tab || self.selected_url() != next_url {
            self.source_epoch += 1;
            self.page = None;
            self.devtools = None;
            self.tools_open = false;
        }
        self.group = tabs["group"].as_str().unwrap_or_default().to_owned();
        self.selected_tab = selected_tab;
        self.selected = selected;
        // A queued follow may be cancelled by a rapid return to the retained
        // frontend. Resume that exact surface and its ACK, not a new epoch.
        self.tools_open = self.tools_wanted
            && !self.tools_target.is_empty()
            && self.selected == self.tools_target;
        self.tabs = entries;
        self.revision += 1;
    }

    fn entry_target(tab: &Value) -> &str {
        match tab.get("target") {
            Some(target) => target.as_str().unwrap_or_default(),
            None => tab["id"].as_str().unwrap_or_default(),
        }
    }
    fn owns_target(&self, target: &str) -> bool {
        !target.is_empty() && self.tabs.iter().any(|tab| Self::entry_target(tab) == target)
    }
    fn selected_url(&self) -> &str {
        self.tabs.iter().find(|tab| tab["id"] == self.selected_tab)
            .and_then(|tab| tab["url"].as_str()).unwrap_or_default()
    }

    fn observed_stop(&mut self) {
        if self.phase == Phase::Stopped { return; }
        self.phase = Phase::Stopped;
        self.ready = false;
        self.clipboard_poisoned = false;
        self.page = None;
        self.devtools = None;
        self.tools_open = false;
        self.error = "Stopped. Profile retained; live drafts and JavaScript memory were lost.".into();
        self.revision += 1;
    }

    fn observation_failed(&mut self, phase: Phase, error: String) {
        if !self.ready { return; }
        self.ready = false;
        self.phase = phase;
        self.error = error;
        self.page = None;
        self.devtools = None;
        self.revision += 1;
    }

    fn queue_failed(&mut self, error: &str) {
        if self.ready {
            self.error = format!("Browser input queue full/disconnected: {error}");
            self.revision += 1;
        }
    }

    fn accept_follow(&mut self, target: &str) -> bool {
        let current = self.tools_wanted && self.selected == target;
        if !current && self.tools_requested_target == target {
            self.tools_requested_target.clear();
        }
        current
    }

    fn needs_follow(&self, target: &str) -> bool {
        self.tools_wanted && self.tools_target != target && self.tools_requested_target != target
    }

    fn tools_opened(&mut self, target: String, follow: bool) {
        // The core's open is idempotent for a retained frontend. Only a real
        // replacement (the previous frontend was closed) starts ACK0 again.
        if self.tools_target != target {
            self.tools_epoch += 1;
            self.devtools = None;
        }
        if !follow {
            self.tools_wanted = true;
        }
        self.tools_target = target;
        self.tools_open = self.tools_wanted && self.selected == self.tools_target;
        self.tools_requested_target.clear();
    }
}
struct Command {
    method: &'static str,
    params: Value,
    /// Internal scheduling flag, never a new wire method.
    follow: bool,
    clipboard: Option<clipboard::Request>,
    context: Option<context_menu::Request>,
}

#[derive(Clone)]
pub struct GroupConfig {
    pub id: String,
    pub saved: super::saved::SavedGroup,
}
pub struct Config {
    pub workspace: String,
    pub machine: String,
    pub viewer: Arc<super::viewer::Viewer>,
    pub group: Option<GroupConfig>,
    pub attached: bool,
    pub visible: bool,
}

pub struct Panel {
    pane: String,
    page_view: Entity<super::chrome::NativePage>,
    chrome: Option<Entity<gpui_shell::ShellRoot>>,
    chrome_copy: u64,
    chrome_runtime: Option<Rc<gpui_shell::ShellRuntime>>,
    workspace: String,
    machine: String,
    viewer: Arc<super::viewer::Viewer>,
    group: Option<GroupConfig>,
    shared: Arc<Mutex<Snapshot>>,
    visible: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    pending_clipboard: Option<clipboard::Pending>,
    context_menu: Option<context_menu::Menu>,
    context_pressed: Option<context_menu::RightPress>,
    _clipboard_focus: Vec<Subscription>,
    commands: mpsc::SyncSender<Command>,
    focus: FocusHandle,
    page_bounds: Rc<Cell<Bounds<Pixels>>>,
    tools_bounds: Rc<Cell<Bounds<Pixels>>>,
    address: String,
    editing_address: bool,
    tools_focus: bool,
    seen: u64,
    last_target: String,
    last_tools_epoch: u64,
    pressed: std::cell::RefCell<std::collections::HashMap<String, String>>,
    finished: Option<mpsc::Receiver<()>>,
    confirm_takeover: bool,
    confirm_stop: bool,
    _refresh: Task<()>,
}

impl Panel {
    pub fn new(workspace: String, machine: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_viewer(workspace, machine, Arc::default(), window, cx)
    }
    pub fn with_viewer(workspace: String, machine: String, viewer: Arc<super::viewer::Viewer>,
        window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::configured(Config { workspace, machine, viewer, group:None, attached:true, visible:true }, window, cx)
    }
    pub fn configured(config: Config, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::start(config, false, None, window, cx)
    }
    fn reconnect(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.invalidate_clipboard();
        if let Some(saved) = self.saved_group() {
            if let Some(group) = &mut self.group { group.saved = saved; }
        }
        self.closed.store(true, Ordering::SeqCst);
        self.shared.lock().unwrap().ready = false;
        let poisoned = {
            let state = self.shared.lock().unwrap();
            state.clipboard_poisoned && state.phase != Phase::Stopped
        };
        let previous = self.finished.take();
        let replacement = Self::start(
            Config { workspace:self.workspace.clone(), machine:self.machine.clone(),
                viewer:self.viewer.clone(), group:self.group.clone(), attached:true,
                visible:self.visible.load(Ordering::Relaxed) },
            force,
            previous,
            window,
            cx,
        );
        replacement.shared.lock().unwrap().clipboard_poisoned = poisoned;
        let pane = self.pane.clone();
        let chrome = self.chrome.take();
        let copy = self.chrome_copy;
        let runtime = self.chrome_runtime.clone();
        *self = replacement;
        if let Some(runtime) = runtime { self.set_chrome(pane, chrome, copy, runtime, cx); }
        self.focus.focus(window, cx);
        cx.notify();
    }
    pub fn set_chrome(&mut self, pane: String, root: Option<Entity<gpui_shell::ShellRoot>>, copy: u64,
        runtime: Rc<gpui_shell::ShellRuntime>, cx: &mut Context<Self>) {
        let changed = self.pane != pane || self.chrome_copy != copy || self.chrome != root;
        if self.pane != pane && !self.pane.is_empty() { super::chrome::remove(&self.pane); }
        self.pane = pane;
        self.chrome = root;
        self.chrome_copy = copy;
        self.chrome_runtime = Some(runtime);
        if changed { self.invalidate_clipboard(); self.refresh_chrome(cx); cx.notify(); }
    }
    fn publish_chrome(&self) -> bool {
        if self.pane.is_empty() { return false; }
        let state = self.shared.lock().unwrap();
        let url = state.selected_url();
        let mut snapshot = json!({"pane":self.pane,"workspace":self.workspace,
            "remote":!self.machine.is_empty(),"ready":state.ready,"phase":format!("{:?}",state.phase),
            "url":url,"selected_tab":state.selected_tab,"tabs":state.tabs,
            "tools_open":state.tools_open,"input_pending":state.input_pending,
            "error":if state.error.is_empty(){None}else{Some(&state.error)}});
        snapshot.as_object_mut().unwrap().extend(self.clipboard_state(&state).as_object().unwrap().clone());
        snapshot.as_object_mut().unwrap().extend(self.context_state(&state).as_object().unwrap().clone());
        super::chrome::publish(&self.pane, snapshot, self.page_view.downgrade())
    }
    fn refresh_chrome(&self, cx: &mut Context<Self>) {
        // Frame-only revisions never cause a JS description rebuild.
        if self.publish_chrome() && let (Some(runtime), Some(root)) = (&self.chrome_runtime, &self.chrome) {
            let _ = runtime.refresh(root, cx);
        }
    }
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }
    pub fn visible(&self, visible: bool) {
        if self.visible.swap(visible, Ordering::Relaxed) && !visible {
            self.invalidate_clipboard();
            for (_, key) in std::mem::take(&mut *self.pressed.borrow_mut()) {
                self.input(self.tools_focus,json!({"type":"key","action":"up","key":key,"code":"","modifiers":0,"repeat":false}));
            }
            let mut s = self.shared.lock().unwrap();
            s.page = None;
            s.devtools = None;
            s.revision += 1;
        }
    }
    /// App commands share the same bounded, controller-owned queue as trusted UI.
    /// A queued reply is not a claim that Chrome already performed the action.
    pub fn dispatch_command(&mut self, name: &str, args: &Value, window: &mut Window, cx: &mut Context<Self>) -> std::result::Result<Value, String> {
        if name == "browser.address_context" {
            return self.address_context_command(args, window, cx);
        }
        if matches!(name, "browser.context_action" | "browser.context_dismiss") {
            return self.context_command(name, args, window, cx);
        }
        if matches!(name, "browser.copy" | "browser.cut" | "browser.paste") {
            return self.clipboard_command(name, window, cx);
        }
        if !self.machine.is_empty() && name != "browser.status" {
            return Err("Remote browser route unavailable: local fallback is forbidden".into());
        }
        if name == "browser.attach" {
            let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
            if force && args.get("confirmed").and_then(Value::as_bool) != Some(true) {
                return Err("needsConfirmation: force takeover requires confirmed:true".into());
            }
            self.reconnect(force, window, cx);
            return Ok(json!({"queued":true}));
        }
        if name == "browser.status" {
            let geometry = |bounds: Bounds<Pixels>| json!({"x":f32::from(bounds.origin.x),"y":f32::from(bounds.origin.y),
                "width":f32::from(bounds.size.width),"height":f32::from(bounds.size.height)});
            let state = self.shared.lock().unwrap();
            let mut snapshot = json!({"workspace":self.workspace,"ready":state.ready,"phase":format!("{:?}",state.phase),
                "group":state.group,"selected_tab":state.selected_tab,"selected":state.selected,"tabs":state.tabs,
                "panel_error":state.error,"input_pending":state.input_pending,
                "browser_pid":state.browser_pid,"browser_instance":state.instance,"window_active":window.is_window_active(),
                "page_focused":self.focus.is_focused(window) && !self.editing_address && !self.tools_focus,
                "page_bounds":geometry(self.page_bounds.get()),"tools_bounds":geometry(self.tools_bounds.get())});
            snapshot.as_object_mut().unwrap().extend(self.clipboard_state(&state).as_object().unwrap().clone());
            snapshot.as_object_mut().unwrap().extend(self.context_state(&state).as_object().unwrap().clone());
            return Ok(snapshot);
        }
        if name == "browser.focus_address" {
            self.invalidate_clipboard();
            if self.chrome_copy != 0 {
                if !crate::mods::send_command_to(self.chrome_copy, "focus_address") {
                    return Err("Browser chrome is not shown".into());
                }
                return Ok(json!({"queued":true,"focus":"address"}));
            }
            for (_, key) in std::mem::take(&mut *self.pressed.borrow_mut()) {
                self.input(self.tools_focus, json!({"type":"key","action":"up","key":key,"code":"","modifiers":0,"repeat":false}));
            }
            self.editing_address = true;
            self.tools_focus = false;
            self.focus.focus(window, cx);
            cx.notify();
            return Ok(json!({"focused":"address"}));
        }
        if name == "browser.stop" && args.get("confirmed").and_then(Value::as_bool) != Some(true) {
            return Err("needsConfirmation: stop requires confirmed:true".into());
        }
        if self.group.is_some() { return self.group_command(name, args, cx); }
        let (selected, tabs) = {
            let state = self.shared.lock().unwrap();
            if !state.ready { return Err("Browser controller is not attached".into()); }
            (state.selected.clone(), state.tabs.clone())
        };
        let target = match args.get("tab") {
            None | Some(Value::Null) => selected.clone(),
            Some(Value::String(target)) => target.clone(),
            _ => return Err("Browser tab must be a string".into()),
        };
        let (method, params, selection): (&'static str, Value, Option<String>) = match name {
            "browser.stop" => ("browser.stop", json!({"confirmed":true}), None),
            "browser.tab.new" => {
                let url = match args.get("url") {
                    None | Some(Value::Null) => "about:blank",
                    Some(Value::String(url)) => url,
                    _ => return Err("Browser URL must be a string".into()),
                };
                ("browser.tab.create", json!({"url":url}), None)
            }
            "browser.tab.next" | "browser.tab.previous" => {
                let index = tabs.iter().position(|tab| tab["id"] == selected).ok_or("No selected browser tab")?;
                let next = if name == "browser.tab.next" { (index + 1) % tabs.len() } else { (index + tabs.len() - 1) % tabs.len() };
                let target = tabs[next]["id"].as_str().ok_or("Invalid browser tab identity")?.to_owned();
                ("browser.tab.select", json!({"target":target}), Some(target))
            }
            "browser.tab.select" | "browser.tab.close" => {
                if !tabs.iter().any(|tab| tab["id"] == target) { return Err("Browser tab does not belong to this workspace".into()); }
                let select = name == "browser.tab.select";
                (if select {"browser.tab.select"} else {"browser.tab.close"}, json!({"target":target}), select.then_some(target))
            }
            "browser.navigate" => ("browser.navigate", json!({"target":selected,"url":args.get("url").and_then(Value::as_str).ok_or("Browser URL must be a string")?}), None),
            "browser.back" | "browser.forward" => ("browser.history", json!({"target":selected,"direction":if name == "browser.back" {"back"} else {"forward"}}), None),
            "browser.reload" => ("browser.reload", json!({"target":selected}), None),
            "browser.devtools.open" => ("browser.devtools.open", json!({"target":selected}), None),
            "browser.devtools.close" => ("browser.devtools.close", json!({"target":selected}), None),
            _ => return Err("Unsupported browser command".into()),
        };
        self.invalidate_clipboard();
        self.commands.try_send(Command { method, params, follow: false, clipboard: None, context: None })
            .map_err(|error| format!("Browser command queue full/disconnected: {error}"))?;
        if let Some(selection) = selection {
            let mut state = self.shared.lock().unwrap();
            state.selected = selection;
            state.page = None;
            state.revision += 1;
        }
        cx.notify();
        Ok(json!({"queued":true}))
    }

    fn command(&self, method: &'static str, params: Value) {
        // Layout/held-key callbacks can arrive after stop or ownership loss.
        // They must neither enqueue work nor overwrite the terminal diagnosis.
        if !self.shared.lock().unwrap().ready {
            return;
        }
        let event = &params["event"];
        if method != "browser.input" || event["type"] == "wheel" || event["type"] == "text"
            || event["action"] == "down" {
            self.invalidate_clipboard();
        }
        if let Err(e) = self.commands.try_send(Command {
            method,
            params,
            follow: false,
            clipboard: None,
            context: None,
        }) {
            let mut s = self.shared.lock().unwrap();
            s.queue_failed(&e.to_string());
        }
    }
    fn target(&self) -> String {
        self.shared.lock().unwrap().selected.clone()
    }
    fn surface(&self, tools: bool) -> Value {
        json!({"kind":if tools {"devtools"} else {"page"},"target":self.target()})
    }
    fn input(&self, tools: bool, event: Value) {
        if self.target().is_empty() { return; }
        self.command(
            "browser.input",
            json!({"surface":self.surface(tools),"event":event}),
        );
    }
    fn viewport(&self, tools: bool, bounds: Bounds<Pixels>) {
        if !tools { self.invalidate_clipboard(); }
        if self.target().is_empty() {
            return;
        }
        self.command("browser.viewport",json!({"surface":self.surface(tools),"width":f32::from(bounds.size.width).clamp(1.,4096.) as u32,"height":f32::from(bounds.size.height).clamp(1.,4096.) as u32,"scale":1}));
    }
    fn menu_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if ev.keystroke.key == "escape" && self.context_menu.is_some() {
            let _ = self.context_command("browser.context_dismiss", &json!({}), window, cx);
            cx.stop_propagation();
        }
    }
    fn key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.chrome.is_some() && !self.focus.is_focused(window) { return; }
        let k = &ev.keystroke;
        if k.key == "escape" && self.context_menu.is_some() {
            let _ = self.context_command("browser.context_dismiss", &json!({}), window, cx);
            cx.stop_propagation();
            return;
        }
        if let Some(name) = clipboard::shortcut(&k.key, &k.modifiers) {
            if !ev.is_held {
                self.context_menu = None;
                self.cancel_clipboard();
                // Release modifiers before the native helper command; their later
                // physical key-up must not invalidate this operation's owner.
                for (_, key) in std::mem::take(&mut *self.pressed.borrow_mut()) {
                    self.input(self.tools_focus, json!({"type":"key","action":"up","key":key,"code":"","modifiers":0,"repeat":false}));
                }
                let _ = self.clipboard_command(name, window, cx);
            }
            cx.stop_propagation();
            return;
        }
        self.invalidate_clipboard();
        // Ctrl+Escape only leaves the streamed surface; never sends Escape to a
        // running debugger or discards a page draft. Existing Herdr actions still
        // resolve through the parent key context, not a second hardcoded map.
        if k.modifiers.control && k.key == "escape" {
            for (_, key) in std::mem::take(&mut *self.pressed.borrow_mut()) {
                self.input(self.tools_focus,json!({"type":"key","action":"up","key":key,"code":"","modifiers":0,"repeat":false}));
            }
            self.editing_address = true;
            self.tools_focus = false;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.editing_address {
            match k.key.as_str() {
                "enter" => {
                    self.command(
                        "browser.navigate",
                        json!({"target":self.target(),"url":self.address}),
                    );
                    self.editing_address = false;
                }
                "backspace" => {
                    self.address.pop();
                }
                _ => {
                    if !k.modifiers.control && !k.modifiers.platform {
                        if let Some(text) = &k.key_char {
                            self.address.push_str(text);
                        }
                    }
                }
            }
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let key = dom_key(&k.key).to_owned();
        let modifiers = mods(&k.modifiers);
        self.input(self.tools_focus,json!({"type":"key","action":"down","key":key,"code":"","modifiers":modifiers,"repeat":ev.is_held}));
        self.pressed.borrow_mut().insert(k.key.clone(), key);
        if let Some(text) = k.key_char.as_ref().filter(|_| modifiers & 7 == 0) {
            self.input(self.tools_focus, json!({"type":"text","text":text}));
        }
        let _ = window;
        cx.stop_propagation();
    }
    fn key_up(&mut self, ev: &KeyUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if let Some(key) = self.pressed.borrow_mut().remove(&ev.keystroke.key) {
            self.input(self.tools_focus,json!({"type":"key","action":"up","key":key,"code":"","modifiers":mods(&ev.keystroke.modifiers),"repeat":false}));
        }
    }
    /// The mod composition slot contains only native streamed surfaces.
    pub fn render_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let tools = self.shared.lock().unwrap().tools_open;
        div().flex().flex_col().size_full().min_h_0()
            .child(self.streamed(false, cx))
            .when(tools, |element| element.child(div().h(px(330.)).w_full().flex().flex_col()
                .border_t_1().border_color(rgb(0x657080)).child(self.streamed(true, cx))))
            .into_any_element()
    }

    fn streamed(&self, tools: bool, cx: &mut Context<Self>) -> AnyElement {
        let bounds = if tools {
            self.tools_bounds.clone()
        } else {
            self.page_bounds.clone()
        };
        let before = bounds.clone();
        let entity = cx.entity().downgrade();
        let input_view = cx.entity();
        let input_focus = self.focus.clone();
        let image = {
            let s = self.shared.lock().unwrap();
            if tools {
                s.devtools.clone()
            } else {
                s.page.clone()
            }
        };
        div().id(if tools {"devtools-surface"} else {"page-surface"}).relative().flex_1().min_h_0().w_full().overflow_hidden().bg(rgb(0xffffff))
            .child(canvas(move |b,_,cx| { if before.replace(b).size != b.size { let _=entity.update(cx,|this,_| this.viewport(tools,b)); } },move |b,_,w,cx| {w.handle_input(&input_focus, ElementInputHandler::new(b,input_view),cx);if let Some(image)=image {let _=w.paint_image(b,b,Corners::default(),image,0,false);} }).size_full())
            .on_mouse_down(MouseButton::Right,cx.listener(move |this,e:&MouseDownEvent,w,cx| this.context_pointer_down(tools,e,w,cx)))
            .on_mouse_up(MouseButton::Right,cx.listener(move |this,e:&MouseUpEvent,w,cx| this.context_pointer_up(tools,e,w,cx)))
            .on_mouse_down(MouseButton::Left,cx.listener(move |this,e:&MouseDownEvent,w,cx| {this.invalidate_clipboard();this.focus.focus(w,cx);this.tools_focus=tools;this.editing_address=false; let b=if tools {this.tools_bounds.get()}else{this.page_bounds.get()}; let p=e.position-b.origin;this.input(tools,json!({"type":"pointer","action":"down","x":f32::from(p.x),"y":f32::from(p.y),"button":"left","buttons":1,"modifiers":mods(&e.modifiers),"click_count":e.click_count}));}))
            .on_mouse_up(MouseButton::Left,cx.listener(move |this,e:&MouseUpEvent,_,_| {let b=if tools {this.tools_bounds.get()}else{this.page_bounds.get()};let p=e.position-b.origin;this.input(tools,json!({"type":"pointer","action":"up","x":f32::from(p.x),"y":f32::from(p.y),"button":"left","buttons":0,"modifiers":mods(&e.modifiers),"click_count":1}));}))
            .on_mouse_move(cx.listener(move |this,e:&MouseMoveEvent,_,_| {if e.pressed_button==Some(MouseButton::Left) {let b=if tools {this.tools_bounds.get()}else{this.page_bounds.get()};let p=e.position-b.origin;this.input(tools,json!({"type":"pointer","action":"move","x":f32::from(p.x),"y":f32::from(p.y),"button":"none","buttons":1,"modifiers":mods(&e.modifiers),"click_count":1}));}}))
            .on_scroll_wheel(cx.listener(move |this,e:&ScrollWheelEvent,_,cx| {let b=if tools {this.tools_bounds.get()}else{this.page_bounds.get()};let p=e.position-b.origin;let d=e.delta.pixel_delta(px(24.));this.input(tools,json!({"type":"wheel","x":f32::from(p.x),"y":f32::from(p.y),"dx":-f32::from(d.x),"dy":-f32::from(d.y),"modifiers":mods(&e.modifiers)}));cx.stop_propagation();})).into_any_element()
    }
}
// Own native text callbacks while focused; never leave a terminal as their
// recipient. Full preedit/IME composition is not advertised by Stage A.
impl EntityInputHandler for Panel {
    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        false
    }
    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
    }
    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus.is_focused(window) {
            return;
        }
        if self.editing_address {
            self.address.push_str(text);
            cx.notify();
        } else {
            self.input(self.tools_focus, json!({"type":"text","text":text}));
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        _: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut s = self.shared.lock().unwrap();
        s.error = "IME composition is not supported in Stage A".into();
        s.revision += 1;
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
fn dom_key(key: &str) -> &str {
    match key {
        "enter" => "Enter",
        "backspace" => "Backspace",
        "escape" => "Escape",
        "tab" => "Tab",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "delete" => "Delete",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "space" => " ",
        "f1" => "F1",
        "f2" => "F2",
        "f3" => "F3",
        "f4" => "F4",
        "f5" => "F5",
        "f6" => "F6",
        "f7" => "F7",
        "f8" => "F8",
        "f9" => "F9",
        "f10" => "F10",
        "f11" => "F11",
        "f12" => "F12",
        "control" => "Control",
        "shift" => "Shift",
        "alt" => "Alt",
        "platform" => "Meta",
        other => other,
    }
}
fn mods(m: &Modifiers) -> u8 {
    u8::from(m.alt) | u8::from(m.control) * 2 | u8::from(m.platform) * 4 | u8::from(m.shift) * 8
}
impl Drop for Panel {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        self.visible.store(false, Ordering::Relaxed);
        self.invalidate_clipboard();
        self.shared.lock().unwrap().ready = false;
        if !self.pane.is_empty() { super::chrome::remove(&self.pane); }
    }
}
impl Render for Panel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (tabs, error, tools, tools_epoch, phase, ready) = {
            let s = self.shared.lock().unwrap();
            (
                s.tabs.clone(),
                s.error.clone(),
                s.tools_open,
                s.tools_epoch,
                s.phase,
                s.ready,
            )
        };
        if self.last_tools_epoch != tools_epoch {
            self.last_tools_epoch = tools_epoch;
            self.tools_bounds.set(Bounds::default());
        }
        let target = self.target();
        if !self.editing_address {
            if let Some(url) = tabs
                .iter()
                .find(|t| t["id"] == target)
                .and_then(|t| t["url"].as_str())
            {
                self.address = url.into();
            }
        }
        let input_pending = self.shared.lock().unwrap().input_pending;
        if self.last_target != target {
            self.last_target = target.clone();
            self.page_bounds.set(Bounds::default());
            self.tools_bounds.set(Bounds::default());
        }
        if let Some(root) = self.chrome.clone() {
            return div().id("workspace-browser").occlude().track_focus(&self.focus)
                .key_context("HerdrBrowser").size_full()
                .capture_key_down(cx.listener(Self::menu_key))
                .on_key_down(cx.listener(Self::key)).on_key_up(cx.listener(Self::key_up))
                .child(root).into_any_element();
        }
        div()
            .id("workspace-browser")
            .occlude()
            .track_focus(&self.focus)
            .key_context("HerdrBrowser")
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x20242b))
            .text_color(rgb(0xffffff))
            .capture_key_down(cx.listener(Self::menu_key))
            .on_key_down(cx.listener(Self::key))
            .on_key_up(cx.listener(Self::key_up))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .p_2()
                    .child(format!("Browser · {}{}", self.machine, self.workspace))
                    .child(
                        div()
                            .id("browser-back")
                            .cursor_pointer()
                            .child("←")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.command(
                                    "browser.history",
                                    json!({"target":this.target(),"direction":"back"}),
                                )
                            })),
                    )
                    .child(
                        div()
                            .id("browser-forward")
                            .cursor_pointer()
                            .child("→")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.command(
                                    "browser.history",
                                    json!({"target":this.target(),"direction":"forward"}),
                                )
                            })),
                    )
                    .child(
                        div()
                            .id("browser-reload")
                            .cursor_pointer()
                            .child("Reload")
                            .on_click(cx.listener(|this, _, _, _| {
                                this.command("browser.reload", json!({"target":this.target()}))
                            })),
                    )
                    .child(
                        div()
                            .id("browser-address")
                            .flex_1()
                            .bg(rgb(0x353b45))
                            .child(self.address.clone())
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.invalidate_clipboard();
                                this.editing_address = true;
                                this.address.clear();
                                this.focus.focus(w, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("browser-tools")
                            .cursor_pointer()
                            .child(if tools {
                                "Hide DevTools"
                            } else {
                                "DevTools ↓"
                            })
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.command(
                                    if tools {
                                        "browser.devtools.close"
                                    } else {
                                        "browser.devtools.open"
                                    },
                                    json!({"target":this.target()}),
                                )
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .p_1()
                    .children(tabs.iter().map(|tab| {
                        let id = tab["id"].as_str().unwrap_or_default().to_owned();
                        let selected = id == target;
                        let close_id=id.clone();
                        div().flex().gap_2()
                            .id(SharedString::from(format!("browser-tab-{id}")))
                            .px_2()
                            .bg(rgb(if selected { 0x475166 } else { 0x20242b }))
                            .child(
                                tab["title"]
                                    .as_str()
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or("Blank")
                                    .to_owned(),
                            )
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.editing_address=false;
                                this.command("browser.tab.select", json!({"target":id}))
                            }))
                            .child(div().id(SharedString::from(format!("browser-close-{close_id}"))).cursor_pointer().child("×").on_click(cx.listener(move |this,_,_,cx| {this.command("browser.tab.close",json!({"target":close_id}));cx.stop_propagation();})))
                    }))
                    .child(div().id("browser-new-tab").child("+").on_click(
                        cx.listener(|this, _, _, _| this.command("browser.tab.create", json!({}))),
                    )),
            )
            .child(div().flex().gap_3().p_1().text_xs().child(match phase {Phase::Detached=>"Saved browser pane — attach to start its selected page",Phase::Starting=>"Connecting to workspace core…",Phase::Running=>"Attached · exclusive controller",Phase::Blocked=>"In use on another desktop",Phase::Revoked=>"Control revoked/disconnected · no automatic reclaim",Phase::Stopped=>"Stopped · profile retained",Phase::Failed=>"Browser unavailable"})
                .when(!ready && phase!=Phase::Starting,|d|d.child(div().id("browser-reconnect").cursor_pointer().child("Reconnect / start").on_click(cx.listener(|this,_,w,cx|this.reconnect(false,w,cx)))))
                .when(matches!(phase,Phase::Blocked|Phase::Revoked),|d|d.child(div().id("browser-takeover").cursor_pointer().child("Force takeover…").on_click(cx.listener(|this,_,_,cx|{this.invalidate_clipboard();this.confirm_takeover=true;cx.notify();}))))
                .when(ready,|d|d.child(div().id("browser-stop").cursor_pointer().child("Stop workspace browser…").on_click(cx.listener(|this,_,_,cx|{this.invalidate_clipboard();this.confirm_stop=true;cx.notify();})))))
            .when(self.confirm_takeover,|d|d.child(div().flex().gap_3().p_2().bg(rgb(0x59442c)).child("Disconnect the other desktop and revoke its input/DevTools?")
                .child(div().id("browser-confirm-takeover").cursor_pointer().child("Confirm takeover").on_click(cx.listener(|this,_,w,cx|this.reconnect(true,w,cx))))
                .child(div().id("browser-cancel-takeover").cursor_pointer().child("Cancel").on_click(cx.listener(|this,_,_,cx|{this.confirm_takeover=false;cx.notify();})))))
            .when(self.confirm_stop,|d|d.child(div().flex().gap_3().p_2().bg(rgb(0x59442c)).child("Stop Chromium? Unsaved drafts and JS memory will be lost. Profile retained; URL restart recovery is not implemented.")
                .child(div().id("browser-confirm-stop").cursor_pointer().child("Confirm stop").on_click(cx.listener(|this,_,_,cx|{this.confirm_stop=false;this.command("browser.stop",json!({"confirmed":true}));cx.notify();})))
                .child(div().id("browser-cancel-stop").cursor_pointer().child("Cancel").on_click(cx.listener(|this,_,_,cx|{this.confirm_stop=false;cx.notify();})))))
            .when(!error.is_empty(), |d| {
                d.child(div().p_2().text_color(rgb(0xffb0a0)).child(error))
            })
            .child(self.streamed(false, cx))
            .when(tools, |d| {
                d.child(
                    div()
                        .h(px(330.))
                        .w_full()
                        .flex()
                        .flex_col()
                        .border_t_1()
                        .border_color(rgb(0x657080))
                        .child(self.streamed(true, cx)),
                )
            })
            .child(div().text_xs().child(
                format!("Ctrl+Escape: focus address without sending Escape · hide keeps Chromium alive · input ACKs pending: {input_pending}"),
            )).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Phase, Snapshot};
    use serde_json::json;

    fn select(s: &mut Snapshot, target: &str) {
        s.tabs(json!({"revision":s.metadata_revision + 1,"selected":target,"tabs":[{"id":"A"},{"id":"B"}]}));
    }

    #[test]
    fn grouped_snapshot_rejects_foreign_and_legacy_catalogs_before_revision_comparison() {
        let mut state = Snapshot::default();
        state.tabs(json!({"group":"pane-a","revision":1,"selected":"pane-a:tab",
            "tabs":[{"id":"pane-a:tab","target":"native-A","url":"https://owned.test/a"}]}));
        for foreign in [json!({"revision":900,"selected":"native-B","tabs":[{"id":"native-B"}]}),
            json!({"group":"pane-b","revision":1000,"selected":"pane-b:tab","tabs":[{"id":"pane-b:tab","target":"native-B"}]})] {
            state.tabs(foreign);
            assert_eq!(state.group,"pane-a");
            assert_eq!(state.selected,"native-A");
            assert_eq!(state.metadata_revision,1);
        }
        state.tabs(json!({"group":"pane-a","revision":2,"selected":"pane-a:tab",
            "tabs":[{"id":"pane-a:tab","target":"native-A","url":"https://owned.test/new"}]}));
        assert_eq!(state.metadata_revision,2);
        assert_eq!(state.selected_url(),"https://owned.test/new");
    }

    #[test]
    fn grouped_metadata_never_uses_a_saved_tab_id_as_a_native_target() {
        let mut state = Snapshot::default();
        state.tabs(json!({"group":"pane-a","revision":1,"selected":"bt-saved",
            "tabs":[{"id":"bt-saved","target":"native-A","url":"https://owned.test/"}]}));
        assert_eq!(state.selected,"native-A");
        assert_eq!(state.selected_tab,"bt-saved");
        assert!(state.owns_target("native-A"));
        assert!(!state.owns_target("bt-saved"));
        state.tabs(json!({"group":"pane-a","revision":2,"selected":"bt-lazy",
            "tabs":[{"id":"bt-lazy","target":null,"url":"https://not-loaded.test/"}]}));
        assert!(state.selected.is_empty(),"unloaded saved entries cannot receive frame/input RPCs");
        assert_eq!(state.selected_tab,"bt-lazy");
        assert_eq!(state.selected_url(),"https://not-loaded.test/");
        assert!(!state.owns_target("bt-lazy"));
        assert!(!state.owns_target(""));
    }

    #[test]
    fn late_observation_failure_cannot_replace_an_observed_stop() {
        for phase in [Phase::Stopped, Phase::Revoked, Phase::Failed] {
            let mut state = Snapshot { phase, error:"observed terminal diagnosis".into(), ..Snapshot::default() };
            state.observation_failed(Phase::Failed,"late group RPC failure".into());
            assert_eq!(state.phase,phase);
            assert_eq!(state.error,"observed terminal diagnosis");
            assert_eq!(state.revision,0);
        }
        let mut active = Snapshot { ready:true, phase:Phase::Running, ..Snapshot::default() };
        active.observation_failed(Phase::Failed,"active group RPC failure".into());
        assert!(!active.ready);
        assert_eq!(active.phase,Phase::Failed);
        assert_eq!(active.revision,1);
    }

    #[test]
    fn late_layout_send_failure_preserves_stopped_or_failed_diagnosis() {
        for phase in [Phase::Stopped, Phase::Failed, Phase::Revoked] {
            let mut s = Snapshot {
                phase,
                error: "terminal diagnosis".into(),
                ..Snapshot::default()
            };
            s.queue_failed("closed channel");
            assert_eq!(s.error, "terminal diagnosis");
            assert_eq!(s.revision, 0);
            assert_eq!(s.phase, phase);
        }
        let mut s = Snapshot {
            ready: true,
            ..Snapshot::default()
        };
        s.queue_failed("full channel");
        assert!(s.error.contains("full channel"));
        assert_eq!(s.revision, 1);
    }

    #[test]
    fn rapid_return_retains_frontend_epoch_and_cancels_only_its_follow_latch() {
        let mut s = Snapshot::default();
        select(&mut s, "A");
        s.tools_opened("A".into(), false);
        let epoch = s.tools_epoch;
        select(&mut s, "B");
        assert!(!s.tools_open);
        assert!(s.needs_follow("B"));
        s.tools_requested_target = "B".into(); // queued, not executed
        select(&mut s, "A");
        assert!(s.tools_open, "return resumes the retained frontend");
        assert!(!s.accept_follow("B")); // control worker cancels stale follow
        assert!(s.tools_requested_target.is_empty());
        assert_eq!(
            s.tools_epoch, epoch,
            "retain original consumption ACK epoch"
        );
        s.tools_opened("A".into(), false); // idempotent manual open, too
        assert_eq!(s.tools_epoch, epoch);
        select(&mut s, "B");
        assert!(
            s.needs_follow("B"),
            "cancelled latch must not suppress next B"
        );
        s.tools_requested_target = "B".into();
        assert!(s.accept_follow("B"));
        s.tools_opened("B".into(), true);
        assert!(s.tools_open);
        assert_eq!(
            s.tools_epoch,
            epoch + 1,
            "actually replaced frontend starts fresh ACK"
        );
        select(&mut s, "A");
        assert!(s.accept_follow("A"));
        s.tools_opened("A".into(), true);
        assert!(s.tools_open);
        assert_eq!(s.tools_epoch, epoch + 2);
        s.tools_requested_target = "A".into();
        assert!(!s.accept_follow("B"));
        assert_eq!(s.tools_requested_target, "A", "never clear a newer request");
    }
}
