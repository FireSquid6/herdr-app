//! Thin native Stage A panel, consuming core-owned browser wire revision 2.
use super::*;
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

#[derive(Default)]
struct Snapshot {
    generation: String,
    selected: String,
    tabs: Vec<Value>,
    page: Option<Arc<RenderImage>>,
    devtools: Option<Arc<RenderImage>>,
    error: String,
    ready: bool,
    tools_open: bool,
    tools_epoch: u64,
    revision: u64,
    metadata_revision: u64,
    input_pending: u64,
}
impl Snapshot {
    fn tabs(&mut self, tabs: Value) {
        let revision = tabs["revision"].as_u64().unwrap_or(0);
        if revision < self.metadata_revision {
            return;
        }
        self.metadata_revision = revision;
        let selected = tabs["selected"].as_str().unwrap_or_default().to_owned();
        if selected != self.selected {
            self.page = None;
            self.devtools = None;
            self.tools_open = false;
        }
        self.selected = selected;
        self.tabs = tabs["tabs"].as_array().cloned().unwrap_or_default();
        self.revision += 1;
    }
}
struct Command {
    method: &'static str,
    params: Value,
}

pub struct Panel {
    workspace: String,
    machine: String,
    shared: Arc<Mutex<Snapshot>>,
    visible: Arc<AtomicBool>,
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
    _refresh: Task<()>,
}

impl Panel {
    pub fn new(workspace: String, machine: String, cx: &mut Context<Self>) -> Self {
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let visible = Arc::new(AtomicBool::new(true));
        let (commands, rx) = mpsc::sync_channel::<Command>(128);
        let state = shared.clone();
        let shown = visible.clone();
        let ws = workspace.clone();
        let host = machine.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                if !host.is_empty() {
                    bail!(
                        "Remote browser needs a dedicated authenticated viewer connection to {host}; local fallback is forbidden"
                    );
                }
                let core = Core::connect_browser()?;
                let negotiated = core.call("browser.negotiate", json!({"revision":2,"client":"herdr-native"}))
                    .map_err(|e| anyhow::anyhow!("Workspace browser unsupported: enable HC_BROWSER_CHROME on a revision-2 core ({e:#})"))?;
                if negotiated["revision"] != 2 {
                    bail!("Unsupported browser wire revision");
                }
                core.call_timeout(
                    "browser.ensure",
                    json!({"workspace":ws}),
                    Duration::from_secs(60),
                )?;
                let acquired = core.call("browser.acquire", json!({"workspace":ws}))?;
                let generation = acquired["generation"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing controller generation"))?
                    .to_owned();
                let lease = json!({"workspace":ws,"generation":generation});
                let tabs = core.call("browser.tabs", lease.clone())?;
                {
                    let mut s = state.lock().unwrap();
                    s.generation = generation.clone();
                    s.ready = true;
                    s.tabs(tabs);
                }
                let frame_core = core.clone();
                let frame_state = state.clone();
                let frame_lease = lease.clone();
                let frame_host = host.clone();
                let frame_ws = ws.clone();
                // Exactly one capture/decode in flight per surface. Control RPCs run
                // on the other thread and do not wait for image decompression.
                let frames = std::thread::spawn(move || {
                    let mut acknowledgements =
                        std::collections::HashMap::<(String, usize, u64), SurfaceState>::new();
                    let mut heartbeat = Instant::now();
                    let mut metadata = Instant::now();
                    loop {
                        if !frame_state.lock().unwrap().ready {
                            break;
                        }
                        if heartbeat.elapsed() > Duration::from_secs(5) {
                            match frame_core.call("browser.heartbeat", frame_lease.clone()) {
                                Ok(reply) => {
                                    let mut s = frame_state.lock().unwrap();
                                    s.input_pending = reply["input_pending"].as_u64().unwrap_or(0);
                                    if let Some(errors) =
                                        reply["input_errors"].as_array().filter(|e| !e.is_empty())
                                    {
                                        s.error = format!(
                                            "Input completion failed (controller retained): {}",
                                            errors
                                                .iter()
                                                .map(|e| format!(
                                                    "receipt {}: {}",
                                                    e["receipt"],
                                                    e["message"].as_str().unwrap_or("input failed")
                                                ))
                                                .collect::<Vec<_>>()
                                                .join("; ")
                                        );
                                    }
                                    s.revision += 1;
                                }
                                Err(e) => {
                                    let mut s = frame_state.lock().unwrap();
                                    s.error = format!("Controller disconnected/revoked: {e:#}");
                                    s.ready = false;
                                    s.page = None;
                                    s.devtools = None;
                                    s.revision += 1;
                                    break;
                                }
                            }
                            heartbeat = Instant::now();
                        }
                        if !shown.load(Ordering::Relaxed) {
                            std::thread::park_timeout(Duration::from_millis(100));
                            continue;
                        }
                        if metadata.elapsed() > Duration::from_secs(1) {
                            if let Ok(tabs) = frame_core.call("browser.tabs", frame_lease.clone()) {
                                let mut s = frame_state.lock().unwrap();
                                if s.ready && s.generation == generation {
                                    s.tabs(tabs);
                                }
                            }
                            metadata = Instant::now();
                        }
                        let (target, tools, ready, tools_epoch) = {
                            let s = frame_state.lock().unwrap();
                            (s.selected.clone(), s.tools_open, s.ready, s.tools_epoch)
                        };
                        if !ready {
                            break;
                        }
                        {
                            let s = frame_state.lock().unwrap();
                            acknowledgements.retain(|(id, kind, epoch), _| {
                                (*kind == 0 || *epoch == tools_epoch)
                                    && s.tabs.iter().any(|t| t["id"] == *id)
                            });
                        }
                        if target.is_empty() {
                            std::thread::park_timeout(Duration::from_millis(100));
                            continue;
                        }
                        for index in 0..if tools { 2 } else { 1 } {
                            let ack_key = (
                                target.clone(),
                                index,
                                if index == 1 { tools_epoch } else { 0 },
                            );
                            let identity = Identity {
                                machine: frame_host.clone(),
                                workspace: frame_ws.clone(),
                                target: target.clone(),
                                generation: generation.clone(),
                                devtools: index == 1,
                            };
                            let tracker =
                                acknowledgements.entry(ack_key.clone()).or_insert_with(|| {
                                    SurfaceState {
                                        identity: Some(identity.clone()),
                                        consumed: 0,
                                    }
                                });
                            let ack = tracker.consumed;
                            let mut p = frame_lease.clone();
                            p["surface"] = json!({"kind":if index == 1 {"devtools"} else {"page"},"target":target});
                            p["ack"] = ack.into();
                            let result = frame_core.call("browser.frame", p).and_then(|r| {
                                if r["generation"] != identity.generation {
                                    bail!("Stale frame generation");
                                }
                                let sequence = r["sequence"]
                                    .as_u64()
                                    .ok_or_else(|| anyhow::anyhow!("Missing frame sequence"))?;
                                if sequence <= ack {
                                    bail!("Nonmonotonic frame sequence");
                                }
                                let image = decode_event(&r["event"])?;
                                Ok((sequence, image))
                            });
                            let mut s = frame_state.lock().unwrap();
                            // Even when a visible selection changes during decode, the
                            // original surface consumed this sequence. Retain its ACK,
                            // never transfer it to the newly selected target/generation.
                            if s.generation == identity.generation && s.ready {
                                if let Ok((sequence, _)) = &result {
                                    tracker.commit(&identity, *sequence);
                                }
                            }
                            if !s.ready || s.generation != identity.generation {
                                tracker.revoke();
                            }
                            if s.selected != identity.target
                                || s.generation != identity.generation
                                || !s.ready
                                || (index == 1 && s.tools_epoch != tools_epoch)
                            {
                                continue;
                            }
                            match result {
                                Ok((sequence, image)) => {
                                    if s.error.starts_with("Frame:") {
                                        s.error.clear();
                                    }
                                    let _ = sequence;
                                    if shown.load(Ordering::Relaxed) {
                                        if index == 0 {
                                            s.page = Some(image);
                                        } else if s.tools_open {
                                            s.devtools = Some(image);
                                        }
                                    }
                                    s.revision += 1;
                                }
                                Err(e) => {
                                    s.error = format!("Frame: {e:#}");
                                    s.revision += 1;
                                }
                            }
                        }
                        std::thread::park_timeout(Duration::from_millis(100));
                    }
                });
                for cmd in rx {
                    let mut params = lease.clone();
                    if let Some(values) = cmd.params.as_object() {
                        for (k, v) in values {
                            params[k] = v.clone();
                        }
                    }
                    match core.call(cmd.method, params) {
                        Ok(_) => {
                            {
                                let mut s = state.lock().unwrap();
                                if s.error.starts_with(cmd.method) {
                                    s.error.clear();
                                    s.revision += 1;
                                }
                            }
                            if cmd.method == "browser.devtools.open" {
                                let mut s = state.lock().unwrap();
                                if !s.tools_open {
                                    s.tools_epoch += 1;
                                }
                                s.tools_open = true;
                            }
                            if cmd.method == "browser.devtools.close" {
                                let mut s = state.lock().unwrap();
                                s.tools_open = false;
                                s.devtools = None;
                            }
                            if cmd.method != "browser.input" && cmd.method != "browser.viewport" {
                                if let Ok(tabs) = core.call("browser.tabs", lease.clone()) {
                                    let mut s = state.lock().unwrap();
                                    s.tabs(tabs);
                                }
                            }
                        }
                        Err(e) => {
                            let mut s = state.lock().unwrap();
                            if matches!(
                                crate::hc::error_kind(&e),
                                Some("not_owner" | "stale_generation")
                            ) {
                                s.ready = false;
                                s.page = None;
                                s.devtools = None;
                            }
                            s.error = format!("{}: {e:#}", cmd.method);
                            s.revision += 1;
                        }
                    }
                }
                {
                    state.lock().unwrap().ready = false;
                }
                let _ = frames.join();
                let _ = core.call("browser.release", lease);
                Ok(())
            })();
            if let Err(e) = result {
                let mut s = state.lock().unwrap();
                s.error = format!("{e:#}");
                s.revision += 1;
            }
        });
        let refresh = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        let revision = this.shared.lock().unwrap().revision;
                        if revision != this.seen {
                            this.seen = revision;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            workspace,
            machine,
            shared,
            visible,
            commands,
            focus: cx.focus_handle(),
            page_bounds: Rc::default(),
            tools_bounds: Rc::default(),
            address: "about:blank".into(),
            editing_address: false,
            tools_focus: false,
            seen: 0,
            last_target: String::new(),
            last_tools_epoch: 0,
            pressed: Default::default(),
            _refresh: refresh,
        }
    }
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }
    pub fn visible(&self, visible: bool) {
        if self.visible.swap(visible, Ordering::Relaxed) && !visible {
            for (_, key) in std::mem::take(&mut *self.pressed.borrow_mut()) {
                self.input(self.tools_focus,json!({"type":"key","action":"up","key":key,"code":"","modifiers":0,"repeat":false}));
            }
            let mut s = self.shared.lock().unwrap();
            s.page = None;
            s.devtools = None;
            s.revision += 1;
        }
    }
    fn command(&self, method: &'static str, params: Value) {
        if let Err(e) = self.commands.try_send(Command { method, params }) {
            let mut s = self.shared.lock().unwrap();
            s.error = format!("Browser input queue full/disconnected: {e}");
            s.revision += 1;
        }
    }
    fn target(&self) -> String {
        self.shared.lock().unwrap().selected.clone()
    }
    fn surface(&self, tools: bool) -> Value {
        json!({"kind":if tools {"devtools"} else {"page"},"target":self.target()})
    }
    fn input(&self, tools: bool, event: Value) {
        self.command(
            "browser.input",
            json!({"surface":self.surface(tools),"event":event}),
        );
    }
    fn viewport(&self, tools: bool, bounds: Bounds<Pixels>) {
        if self.target().is_empty() {
            return;
        }
        self.command("browser.viewport",json!({"surface":self.surface(tools),"width":f32::from(bounds.size.width).clamp(1.,4096.) as u32,"height":f32::from(bounds.size.height).clamp(1.,4096.) as u32,"scale":1}));
    }
    fn key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
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
            .on_mouse_down(MouseButton::Left,cx.listener(move |this,e:&MouseDownEvent,w,cx| {this.focus.focus(w,cx);this.tools_focus=tools;this.editing_address=false; let b=if tools {this.tools_bounds.get()}else{this.page_bounds.get()}; let p=e.position-b.origin;this.input(tools,json!({"type":"pointer","action":"down","x":f32::from(p.x),"y":f32::from(p.y),"button":"left","buttons":1,"modifiers":mods(&e.modifiers),"click_count":e.click_count}));}))
            .on_mouse_up(MouseButton::Left,cx.listener(move |this,e:&MouseUpEvent,_,_| {let b=if tools {this.tools_bounds.get()}else{this.page_bounds.get()};let p=e.position-b.origin;this.input(tools,json!({"type":"pointer","action":"up","x":f32::from(p.x),"y":f32::from(p.y),"button":"left","buttons":0,"modifiers":mods(&e.modifiers),"click_count":1}));}))
            .on_mouse_move(cx.listener(move |this,e:&MouseMoveEvent,_,_| {if e.pressed_button.is_some() {let b=if tools {this.tools_bounds.get()}else{this.page_bounds.get()};let p=e.position-b.origin;this.input(tools,json!({"type":"pointer","action":"move","x":f32::from(p.x),"y":f32::from(p.y),"button":"none","buttons":1,"modifiers":mods(&e.modifiers),"click_count":1}));}}))
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
        self.visible.store(false, Ordering::Relaxed);
        self.shared.lock().unwrap().ready = false;
    }
}
impl Render for Panel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (tabs, error, tools, tools_epoch) = {
            let s = self.shared.lock().unwrap();
            (s.tabs.clone(), s.error.clone(), s.tools_open, s.tools_epoch)
        };
        if self.last_tools_epoch != tools_epoch {
            self.last_tools_epoch = tools_epoch;
            self.tools_bounds.set(Bounds::default());
        }
        let target = self.target();
        let input_pending = self.shared.lock().unwrap().input_pending;
        if self.last_target != target {
            self.last_target = target.clone();
            self.page_bounds.set(Bounds::default());
            self.tools_bounds.set(Bounds::default());
        }
        div()
            .id("workspace-browser")
            .occlude()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x20242b))
            .text_color(rgb(0xffffff))
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
                        div()
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
                                this.command("browser.tab.select", json!({"target":id}))
                            }))
                    }))
                    .child(div().id("browser-new-tab").child("+").on_click(
                        cx.listener(|this, _, _, _| this.command("browser.tab.create", json!({}))),
                    )),
            )
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
            ))
    }
}
