use super::{bridge::Callbacks, surface::Geometry};
use cef::{ImplBrowser, ImplBrowserHost, ImplFrame};
use gpui::{prelude::*, *};
use gpui_shell::{ShellRoot, ShellRuntime};
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

pub struct BrowserOptions {
    pub workspace: String,
    pub pane: String,
    pub url: String,
    pub tabs: crate::browser_tabs::BrowserTabs,
    pub profile_root: PathBuf,
    pub remote: bool,
}
pub enum BrowserEvent {
    Navigated { tab: String, url: String },
}
impl EventEmitter<BrowserEvent> for BrowserPane {}
#[derive(Default)]
struct Surface {
    image: Option<Arc<RenderImage>>,
    bounds: Bounds<Pixels>,
}
struct InactivePage {
    uploads: u64,
    surface: Rc<RefCell<Surface>>,
    shared: Arc<Callbacks>,
    browser: Option<cef::Browser>,
    status: super::bridge::LoadStatus,
    geometry: Geometry,
    url: String,
    error: Option<String>,
}

struct AddressContext {
    token: u64,
    owner: super::context_menu::Identity,
    focus: FocusHandle,
    selection: bool,
}
pub struct BrowserPane {
    editing_generation: u64,
    page_context: Option<super::context_menu::PageContext>,
    address_context: Option<AddressContext>,
    can_open_link_in_tab: bool,
    uploads: u64,
    tabs: crate::browser_tabs::BrowserTabs,
    inactive: std::collections::HashMap<String, InactivePage>,
    options: BrowserOptions,
    surface: Rc<RefCell<Surface>>,
    shared: Arc<Callbacks>,
    browser: Option<cef::Browser>,
    focus: FocusHandle,
    page_focus: FocusHandle,
    closed: bool,
    visible: bool,
    status: super::bridge::LoadStatus,
    page_view: Entity<crate::browser_chrome::NativePage>,
    chrome: Option<Entity<ShellRoot>>,
    chrome_runtime: Rc<ShellRuntime>,
    chrome_copy: u64,
    geometry: Geometry,
    url: String,
    error: Option<String>,
}
impl BrowserPane {
    pub fn new(mut options: BrowserOptions, runtime: Rc<ShellRuntime>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.on_release_in(window, |this, window, _| {
            this.evict_inactive_images(window);
            this.begin_close();
            if let Some(image) = this.surface.borrow_mut().image.take() {
                let _ = window.drop_image(image);
            }
        })
        .detach();
        // The root owns shortcut containment, but programmatic pane activation
        // must target the actual page input descendant rather than that ancestor.
        let focus = cx.focus_handle();
        let page_focus = cx.focus_handle();
        cx.on_focus(&focus, window, |this, window, cx| {
            if !this.closed {
                this.page_focus.focus(window, cx);
            }
        })
        .detach();
        cx.on_focus(&page_focus, window, |this, _, _| {
            this.shared.editing_active.store(true, Ordering::Release);
            if let Some(host) = this.host() {
                host.set_focus(1);
            }
        })
        .detach();
        cx.on_blur(&page_focus, window, |this, _, cx| {
            this.invalidate_editing();
            this.refresh_chrome(cx);
            if let Some(host) = this.host() {
                host.set_focus(0);
            }
        })
        .detach();
        tracing::info!(workspace = %options.workspace, pane = %options.pane, remote = options.remote, "native browser pane constructed");
        options.tabs.ensure(&options.url);
        let url = crate::settings::browser_target(options.tabs.url().unwrap_or(&options.url));
        cx.spawn_in(window, async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let Some(view) = view.upgrade() else {
                    break;
                };
                if cx
                    .update_window_entity(&view, |view, window, cx| {
                        view.tick(window, cx);
                        view.closed
                    })
                    .unwrap_or(true)
                {
                    break;
                }
            }
        })
        .detach();
        let error = (!super::policy::local_allowed(options.remote)).then(|| "Remote browser route unavailable. Move this pane to a local workspace; direct remote networking is blocked until a verified route is configured.".into());
        let owner = cx.entity().downgrade();
        let page_view = cx.new(|_| crate::browser_chrome::NativePage(owner));
        let view = Self {
            uploads: 0,
            tabs: options.tabs.clone(),
            inactive: Default::default(),
            options,
            surface: Rc::new(RefCell::new(Surface::default())),
            shared: Arc::new(Callbacks::default()),
            browser: None,
            focus,
            page_focus,
            closed: false,
            visible: true,
            status: super::bridge::LoadStatus::default(),
            page_view,
            chrome: None,
            chrome_runtime: runtime,
            chrome_copy: 0,
            geometry: Geometry::default(),
            url,
            error,
            editing_generation: 0,
            page_context: None,
            address_context: None,
            can_open_link_in_tab: true,
        };
        view.publish_chrome();
        view
    }
    fn evict_inactive_images(&self, window: &mut Window) {
        for page in self.inactive.values() {
            if let Some(image) = page.surface.borrow_mut().image.take() { let _ = window.drop_image(image); }
        }
    }
    fn close_inactive(page: InactivePage) {
        page.shared.invalidate_editing();
        page.shared.hidden.store(true, Ordering::Release);
        page.shared.closing.store(true, Ordering::Release);
        page.shared.mailbox.close();
        if let Some(browser) = page.browser {
            browser.stop_load();
            if let Some(host) = browser.host() { host.set_focus(0); host.close_browser(1); }
        }
    }
    /// Reconcile the durable tab list without recreating any loaded page. Only
    /// the selected page is ever eligible for creation or frame upload.
    pub fn sync_tabs(&mut self, mut tabs: crate::browser_tabs::BrowserTabs, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        tabs.ensure(url);
        if self.closed { return; }
        let changed = self.tabs != tabs;
        if tabs.selected != self.tabs.selected {
            // Revoke capabilities BEFORE swapping the native browser/callback owner.
            self.invalidate_editing();
            let was_visible = self.visible;
            self.set_visible(false, window, cx);
            let next = self.inactive.remove(&tabs.selected).unwrap_or_else(|| InactivePage {
                uploads: 0,
                surface: Rc::new(RefCell::new(Surface::default())),
                shared: Arc::new(Callbacks::default()), browser: None,
                status: Default::default(), geometry: Default::default(),
                url: tabs.url().unwrap_or(url).into(), error: None,
            });
            let old = InactivePage {
                uploads: std::mem::replace(&mut self.uploads, next.uploads),
                surface: std::mem::replace(&mut self.surface, next.surface),
                shared: std::mem::replace(&mut self.shared, next.shared),
                browser: std::mem::replace(&mut self.browser, next.browser),
                status: std::mem::replace(&mut self.status, next.status),
                geometry: std::mem::replace(&mut self.geometry, next.geometry),
                url: std::mem::replace(&mut self.url, next.url),
                error: std::mem::replace(&mut self.error, next.error),
            };
            self.inactive.insert(self.tabs.selected.clone(), old);
            self.editing_generation = self.shared.editing_generation.load(Ordering::Acquire);
            self.tabs = tabs.clone();
            self.set_visible(was_visible, window, cx);
            if was_visible && self.page_focus.is_focused(window) {
                if let Some(host) = self.host() { host.set_focus(1); }
            }
            crate::mods::deliver_command_to(self.chrome_copy, "navigated");
        }
        let removed: Vec<_> = self.inactive.keys().filter(|id| !tabs.items.iter().any(|t| &t.id == *id)).cloned().collect();
        for id in removed {
            if let Some(page) = self.inactive.remove(&id) {
                if let Some(image) = page.surface.borrow_mut().image.take() { let _ = window.drop_image(image); }
                Self::close_inactive(page);
            }
        }
        self.tabs = tabs;
        if self.tabs.url().is_some_and(|url| url != self.url) {
            self.navigate(self.tabs.url().unwrap().into(), window, cx);
        }
        if changed { self.refresh_chrome(cx); cx.notify(); }
    }
    /// Hidden pages retain their last atlas image and process lifecycle/navigation
    /// events, but do not create browsers, accept paint copies or upload images.
    pub fn set_visible(&mut self, visible: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.visible == visible {
            return;
        }
        tracing::info!(pane = %self.options.pane, tab = %self.tabs.selected, visible, "native browser pane visibility changed");
        self.visible = visible;
        self.shared.hidden.store(!visible, Ordering::Release);
        if let Some(host) = self.host() {
            host.was_hidden(i32::from(!visible));
            if visible {
                host.was_resized();
                host.invalidate(cef::PaintElementType::VIEW);
            } else {
                host.set_focus(0);
            }
        }
        if !visible {
            self.invalidate_editing();
            self.shared.mailbox.take();
        }
        self.publish_chrome();
        cx.notify();
    }
    /// Explicit ownership teardown: frame references may keep this entity alive.
    /// CEF's runtime registry retains the browser only until OnBeforeClose.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.evict_inactive_images(window);
        self.begin_close();
        self.chrome = None;
        if let Some(image) = self.surface.borrow_mut().image.take() {
            let _ = window.drop_image(image);
        }
        cx.notify();
    }
    fn begin_close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.invalidate_editing();
        for (_, page) in self.inactive.drain() {
            Self::close_inactive(page);
        }
        crate::browser_chrome::remove(&self.options.pane);
        self.shared.hidden.store(true, Ordering::Release);
        self.shared.closing.store(true, Ordering::Release);
        self.shared.mailbox.close();
        if let Ok(mut navigated) = self.shared.navigated.lock() {
            navigated.take();
        }
        if let Some(browser) = self.browser.take() {
            browser.stop_load();
            if let Some(host) = browser.host() {
                host.set_focus(0);
                host.close_browser(1);
            }
        }
    }
    pub fn current_url(&self) -> String {
        self.url.clone()
    }
    pub fn navigate(&mut self, url: String, _window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.options.remote {
            return;
        }
        self.invalidate_editing();
        self.url = crate::settings::browser_target(&url);
        if let Some(tab) = self.tabs.items.iter_mut().find(|t|t.id == self.tabs.selected) { tab.url = self.url.clone(); }
        self.refresh_chrome(cx);
        crate::mods::deliver_command_to(self.chrome_copy, "navigated");
        if let Some(frame) = self.browser.as_ref().and_then(|b| b.main_frame()) {
            frame.load_url(Some(&self.url.as_str().into()));
        }
        cx.notify();
    }
    pub fn back(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.invalidate_editing();
        self.refresh_chrome(cx);
        if let Some(b) = &self.browser {
            b.go_back();
        }
    }
    pub fn forward(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.invalidate_editing();
        self.refresh_chrome(cx);
        if let Some(b) = &self.browser {
            b.go_forward();
        }
    }
    pub fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.invalidate_editing();
        self.refresh_chrome(cx);
        if let Some(b) = &self.browser {
            b.reload();
        }
    }
    pub fn stop(&mut self, _: &mut Window, _: &mut Context<Self>) {
        if let Some(b) = &self.browser {
            b.stop_load();
        }
    }
    pub fn focus_address(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || !self.visible {
            return;
        }
        self.invalidate_editing();
        if let Some(host) = self.host() {
            host.set_focus(0);
        }
        self.refresh_chrome(cx);
        crate::mods::deliver_command_to(self.chrome_copy, "focus_address");
        cx.notify();
    }
    fn publish_chrome(&self) {
        crate::browser_chrome::publish(&self.options.pane, serde_json::json!({
            "pane": self.options.pane, "workspace": self.options.workspace,
            "url": self.url, "loading": self.status.loading,
            "selected_tab": self.tabs.selected, "active_tab": self.tabs.selected,
            "tabs": self.tabs.items.iter().map(|tab| {
                let status = if tab.id == self.tabs.selected { Some(&self.status) } else { self.inactive.get(&tab.id).map(|p| &p.status) };
                serde_json::json!({"id":tab.id,"url":tab.url,"title":status.map(|s|s.title.as_str()).filter(|s|!s.is_empty()).unwrap_or(&tab.url),"loading":status.is_some_and(|s|s.loading),"favicon":status.and_then(|s|s.favicon.asset.as_ref()),
                    "loaded":if tab.id == self.tabs.selected { self.browser.is_some() } else { self.inactive.get(&tab.id).is_some_and(|p|p.browser.is_some()) },
                    "visible":tab.id == self.tabs.selected && self.visible,
                    "browser_id":if tab.id == self.tabs.selected { self.browser.as_ref().map(|b|b.identifier()) } else { self.inactive.get(&tab.id).and_then(|p|p.browser.as_ref()).map(|b|b.identifier()) },
                    "frame_copies":if tab.id == self.tabs.selected { self.shared.mailbox.counts().0 } else { self.inactive.get(&tab.id).map_or(0, |p|p.shared.mailbox.counts().0) },
                    "frame_uploads":if tab.id == self.tabs.selected { self.uploads } else { self.inactive.get(&tab.id).map_or(0,|p|p.uploads) }})
            }).collect::<Vec<_>>(),
            "can_back": self.status.can_back, "can_forward": self.status.can_forward,
            "remote": self.options.remote, "closed": self.closed, "available": true,
            "error": self.error.as_ref().or(self.status.error.as_ref()),
            "owner": self.browser.as_ref().map(|browser| browser.identifier().to_string()),
            "can_open_link_in_tab": self.can_open_link_in_tab,
            "clipboard_available": false,
            "context": self.page_context.as_ref().map(|menu| serde_json::json!({
                "source": "page", "token": menu.token.to_string(), "link": menu.link, "selection": menu.selection,
                "editable": menu.editable, "can_copy": menu.can_copy,
                "can_cut": menu.can_cut, "can_paste": menu.can_paste,
            })).or_else(|| self.address_context.as_ref().map(|menu| serde_json::json!({
                "source": "address", "token": menu.token.to_string(), "link": "", "selection": "",
                "editable": true, "can_copy": menu.selection, "can_cut": menu.selection, "can_paste": true,
            }))),
        }), self.page_view.downgrade());
    }
    /// Chrome is an ordinary app-owned mod copy, including hot reload and close.
    pub fn set_chrome(&mut self, root: Option<Entity<ShellRoot>>, copy: u64, cx: &mut Context<Self>) {
        if self.chrome_copy == copy { return; }
        self.invalidate_editing();
        self.chrome = root;
        self.chrome_copy = copy;
        self.refresh_chrome(cx);
        crate::mods::deliver_command_to(copy, "navigated");
        cx.notify();
    }

    fn host(&self) -> Option<cef::BrowserHost> {
        if self.closed {
            return None;
        }
        self.browser.as_ref()?.host()
    }
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed { return; }
        let mut changed = false;
        for (id, page) in &mut self.inactive {
            if let Some(url) = page.shared.navigated.lock().ok().and_then(|mut n| n.take()) {
                page.url = url.clone();
                if let Some(tab) = self.tabs.items.iter_mut().find(|t| &t.id == id) { tab.url = url.clone(); }
                cx.emit(BrowserEvent::Navigated { tab: id.clone(), url });
                changed = true;
            }
            if let Ok(status) = page.shared.status.lock() {
                if *status != page.status { page.status = status.clone(); changed = true; }
            }
            page.shared.mailbox.take();
        }
        if changed { self.refresh_chrome(cx); cx.notify(); }
        if self.error.is_some() { return; }
        if self.visible && self.browser.is_none() {
            match super::create_page(&self.options, self.shared.clone(), &self.url) {
                Ok(browser) => {
                    if let Some(browser) = &browser { tracing::info!(pane = %self.options.pane, tab = %self.tabs.selected, browser_id = browser.identifier(), "browser tab live page created"); }
                    self.browser = browser;
                    if self.page_focus.is_focused(window) {
                        if let Some(host) = self.host() {
                            host.set_focus(1);
                        }
                    }
                    self.refresh_chrome(cx);
                }
                Err(error) => {
                    tracing::error!(pane = %self.options.pane, %error, "native browser page creation failed");
                    self.error = Some(error.to_string());
                    self.refresh_chrome(cx);
                    cx.notify();
                    return;
                }
            }
        }
        let geometry = *self
            .shared
            .geometry
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.visible && geometry != self.geometry {
            if let Some(host) = self.host() {
                if geometry.scale != self.geometry.scale {
                    host.notify_screen_info_changed();
                }
                host.was_resized();
            }
            self.geometry = geometry;
        }
        self.drain_editing(window, cx);
        let navigated = self.shared.navigated.lock().ok().and_then(|mut n| n.take());
        if let Some(url) = navigated {
            self.invalidate_editing();
            self.url = url.clone();
            if let Some(tab) = self.tabs.items.iter_mut().find(|t|t.id == self.tabs.selected) { tab.url = url.clone(); }
            self.refresh_chrome(cx);
            crate::mods::deliver_command_to(self.chrome_copy, "navigated");
            cx.emit(BrowserEvent::Navigated { tab: self.tabs.selected.clone(), url });
        }
        let status = self.shared.status.lock().ok().map(|status| status.clone());
        if let Some(status) = status {
            if status != self.status {
                self.status = status;
                self.refresh_chrome(cx);
                cx.notify();
            }
        }
        while self.shared.mailbox.wake.try_recv().is_ok() {}
        if let Some(frame) = self.shared.mailbox.take() {
            if self.visible && geometry.accepts(frame.width, frame.height) {
                if let Some(buffer) =
                    image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
                {
                    let image = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
                        buffer
                    )]));
                    if let Some(old) = self.surface.borrow_mut().image.replace(image) {
                        let _ = window.drop_image(old);
                    }
                    self.uploads += 1;
                    self.page_view.update(cx, |_, cx| cx.notify());
                }
            }
        }
        self.publish_chrome();
    }
    fn mouse(&self, point: Point<Pixels>) -> cef::MouseEvent {
        let origin = self.surface.borrow().bounds.origin;
        let (x, y) = self.geometry.local_point(
            point.x.into(),
            point.y.into(),
            origin.x.into(),
            origin.y.into(),
        );
        cef::MouseEvent { x, y, modifiers: 0 }
    }
}
impl BrowserPane {
    fn refresh_chrome(&self, cx: &mut Context<Self>) {
        self.publish_chrome();
        if let Some(root) = &self.chrome { let _ = self.chrome_runtime.refresh(root, cx); }
    }
    /// Materialized only inside a trusted mod's BrowserPage slot. No toolbar,
    /// address editor or surrounding layout is hardcoded here.
    pub fn render_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.closed || self.error.is_some() { return div().into_any_element(); }
        let pre = self.surface.clone();
        let paint = self.surface.clone();
        let shared = self.shared.clone();
        let content = div()
            .id("browser")
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&self.page_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.dismiss_context(cx);
                    this.page_focus.focus(window, cx);
                    if let Some(host) = this.host() {
                        host.set_focus(1);
                        host.send_mouse_click_event(
                            Some(&this.mouse(event.position)),
                            cef::MouseButtonType::LEFT,
                            0,
                            event.click_count as i32,
                        );
                    }
                }),
            )
            .on_mouse_down(MouseButton::Right, cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.dismiss_context(cx);
                this.page_focus.focus(window, cx);
                this.shared.editing_active.store(true, Ordering::Release);
                if let Some(host) = this.host() {
                    host.set_focus(1);
                    host.send_mouse_click_event(Some(&this.mouse(event.position)), cef::MouseButtonType::RIGHT, 0, 1);
                }
            }))
            .on_mouse_up(MouseButton::Right, cx.listener(|this, event: &MouseUpEvent, _, _| {
                if let Some(host) = this.host() {
                    host.send_mouse_click_event(Some(&this.mouse(event.position)), cef::MouseButtonType::RIGHT, 1, 1);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, _| {
                    if let Some(host) = this.host() {
                        host.send_mouse_click_event(
                            Some(&this.mouse(event.position)),
                            cef::MouseButtonType::LEFT,
                            1,
                            event.click_count as i32,
                        );
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                if let Some(host) = this.host() {
                    host.send_mouse_move_event(Some(&this.mouse(event.position)), 0);
                }
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, _| {
                if let Some(host) = this.host() {
                    let delta = event.delta.pixel_delta(px(20.));
                    host.send_mouse_wheel_event(
                        Some(&this.mouse(event.position)),
                        f32::from(delta.x) as i32,
                        f32::from(delta.y) as i32,
                    );
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.dismiss_context(cx);
                if event.keystroke.modifiers.control && !event.keystroke.modifiers.shift && !event.keystroke.modifiers.alt && !event.keystroke.modifiers.platform {
                    let command = match event.keystroke.key.as_str() { "c" => Some("copy"), "x" => Some("cut"), "v" => Some("paste"), _ => None };
                    if let Some(command) = command {
                        let _ = this.edit(command, window, cx);
                        cx.stop_propagation();
                        return;
                    }
                }
                if let Some(host) = this.host() {
                    if let Some(code) = super::surface::logical_key(&event.keystroke.key) {
                        host.send_key_event(Some(&cef::KeyEvent {
                            type_: cef::KeyEventType::RAWKEYDOWN,
                            windows_key_code: code,
                            modifiers: key_modifiers(&event.keystroke.modifiers),
                            ..Default::default()
                        }));
                    }
                    if !event.keystroke.modifiers.control
                        && !event.keystroke.modifiers.platform
                        && !event.keystroke.modifiers.alt
                    {
                        if let Some(text) = &event.keystroke.key_char {
                            super::bridge::text(&host, text);
                        }
                    }
                }
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, _| {
                if let (Some(host), Some(code)) = (
                    this.host(),
                    super::surface::logical_key(&event.keystroke.key),
                ) {
                    host.send_key_event(Some(&cef::KeyEvent {
                        type_: cef::KeyEventType::KEYUP,
                        windows_key_code: code,
                        modifiers: key_modifiers(&event.keystroke.modifiers),
                        ..Default::default()
                    }));
                }
            }))
            .child(
                canvas(
                    move |bounds, window, _| {
                        pre.borrow_mut().bounds = bounds;
                        if let Ok(mut geometry) = shared.geometry.lock() {
                            *geometry = Geometry {
                                width: f32::from(bounds.size.width)
                                    .round()
                                    .clamp(1., 4096. / window.scale_factor().max(1.))
                                    as i32,
                                height: f32::from(bounds.size.height)
                                    .round()
                                    .clamp(1., 4096. / window.scale_factor().max(1.))
                                    as i32,
                                scale: window.scale_factor(),
                            };
                        }
                    },
                    move |bounds, _, window, _| {
                        if let Some(image) = &paint.borrow().image {
                            let _ = window.paint_image(
                                bounds,
                                bounds,
                                Corners::default(),
                                image.clone(),
                                0,
                                false,
                            );
                        }
                    },
                )
                .size_full(),
            );
        content.into_any_element()
    }
}
impl Render for BrowserPane {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().track_focus(&self.focus).size_full().overflow_hidden()
            .when_some(self.chrome.clone(), |el, root| el.child(root))

    }
}

impl Focusable for BrowserPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Drop for BrowserPane {
    fn drop(&mut self) {
        self.begin_close();
        // Explicit close/on_release_in evict the image when a window exists;
        // window teardown handles the atlas if the window is already gone.
    }
}

// Chromium cef_event_flags_t values; logical keys only, not physical scan codes.
fn key_modifiers(modifiers: &Modifiers) -> u32 {
    u32::from(modifiers.shift) * 2
        | u32::from(modifiers.control) * 4
        | u32::from(modifiers.alt) * 8
        | u32::from(modifiers.platform) * 128
}

impl BrowserPane {
    fn reconcile_editing_generation(&mut self) {
        let current = self.shared.editing_generation.load(Ordering::Acquire);
        if current != self.editing_generation {
            self.invalidate_editing();
        }
    }
    fn invalidate_editing(&mut self) {
        self.page_context = None;
        self.address_context = None;
        self.shared.invalidate_editing();
        self.editing_generation = self.shared.editing_generation.load(Ordering::Acquire);
    }
    fn dismiss_context(&mut self, cx: &mut Context<Self>) {
        if let Ok(mut menu) = self.shared.page_context.lock() { menu.take(); }
        let page = self.page_context.take().is_some();
        let address = self.address_context.take().is_some();
        if page || address { self.refresh_chrome(cx); }
    }
    fn focused_frame(&self) -> Option<(cef::Frame, super::context_menu::Identity)> {
        if self.closed || !self.visible || self.options.remote || self.shared.closed.load(Ordering::Acquire) { return None; }
        let browser = self.browser.as_ref()?;
        let frame = browser.focused_frame()?;
        if frame.is_valid() != 1 { return None; }
        let identity = super::context_menu::identity(browser, &frame);
        Some((frame, identity))
    }
    /// Public page-level seam for both commands and native editing shortcuts.
    /// Address editing uses the existing GPUI input actions, not page IPC.
    pub fn edit(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        self.reconcile_editing_generation();
        if self.closed || !self.visible || self.options.remote || !self.focus.contains_focused(window, cx) {
            return Err("Clipboard target is no longer focused".into());
        }
        if !self.page_focus.is_focused(window) {
            let focus = window.focused(cx).ok_or("Address editor lost focus")?;
            // Dispatch synchronously to this exact editor, never a later focus owner.
            match command {
                "copy" => focus.dispatch_action(&gpui_base::input::Copy, window, cx),
                "cut" => focus.dispatch_action(&gpui_base::input::Cut, window, cx),
                "paste" => focus.dispatch_action(&gpui_base::input::Paste, window, cx),
                _ => return Err("Unknown clipboard action".into()),
            }
            return Ok(());
        }
        self.dismiss_context(cx);
        Err("Page clipboard temporarily unavailable pending native bridge integration and runtime acceptance".into())
    }
    /// The tab owner may consume a link capability and open the returned URL in
    /// a real browser tab. Never implement this with a Herdr workspace tab.
    pub fn take_context_link(&mut self, token: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        self.reconcile_editing_generation();
        if !self.can_open_link_in_tab || !self.page_focus.is_focused(window) { return Err("Browser tab capability unavailable".into()); }
        let (_, owner) = self.focused_frame().ok_or("Page menu owner closed")?;
        let menu = super::context_menu::PageContext::take(&mut self.page_context, token, &owner, self.editing_generation);
        self.refresh_chrome(cx);
        let menu = menu?;
        if !super::policy::allowed_navigation(&menu.link) { return Err("Link navigation unavailable".into()); }
        Ok(menu.link)
    }
    /// Acquire an address-menu capability for the current native browser and
    /// exact GPUI editor focus. Queued chrome callbacks include the owner they
    /// were rendered for, so they cannot open a menu for a replacement tab.
    pub fn address_context(&mut self, owner: &str, selection: bool, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        self.reconcile_editing_generation();
        if !self.visible || self.closed || self.options.remote || self.page_focus.is_focused(window) || !self.focus.contains_focused(window, cx) {
            return Err("Address editor is no longer focused".into());
        }
        let (_, identity) = self.focused_frame().ok_or("Address owner unavailable")?;
        if identity.browser.to_string() != owner { return Err("Address owner changed".into()); }
        let focus = window.focused(cx).ok_or("Address editor lost focus")?;
        self.page_context = None;
        let token = super::context_menu::next_token();
        self.address_context = Some(AddressContext { token, owner: identity, focus, selection });
        self.refresh_chrome(cx);
        Ok(())
    }
    /// Consume the one-shot page-menu capability. The pane name in a command
    /// is only a lookup key: actual ownership is the native browser + frame.
    pub fn context_action(&mut self, token: &str, action: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        self.reconcile_editing_generation();
        if let Some(menu) = self.address_context.take() {
            self.refresh_chrome(cx);
            if menu.token.to_string() != token || !menu.focus.is_focused(window) || self.focused_frame().is_none_or(|(_, owner)| owner != menu.owner) {
                return Err("Stale address menu".into());
            }
            match action {
                "copy" | "cut" | "paste" => self.edit(action, window, cx)?,
                "select_all" => menu.focus.dispatch_action(&gpui_base::input::SelectAll, window, cx),
                "dismiss" => {},
                _ => return Err("Address action unavailable".into()),
            }
            return Ok(());
        }
        if !self.page_focus.is_focused(window) { self.invalidate_editing(); return Err("Page menu owner lost focus".into()); }
        let (frame, current) = self.focused_frame().ok_or("Page menu owner closed")?;
        let menu = super::context_menu::PageContext::take(&mut self.page_context, token, &current, self.editing_generation);
        self.refresh_chrome(cx);
        let menu = menu?;
        match action {
            "dismiss" => {},
            "copy_link" if !menu.link.is_empty() => cx.write_to_clipboard(ClipboardItem::new_string(menu.link)),
            "open_link" if super::policy::allowed_navigation(&menu.link) => self.navigate(menu.link, window, cx),
            "copy" if menu.can_copy => { self.edit("copy", window, cx)?; },
            "cut" if menu.can_cut && menu.editable => { self.edit("cut", window, cx)?; },
            "paste" if menu.can_paste && menu.editable => { self.edit("paste", window, cx)?; },
            "back" => self.back(window, cx),
            "forward" => self.forward(window, cx),
            "reload" => self.reload(window, cx),
            "select_all" => frame.select_all(),
            _ => return Err("Page action unavailable".into()),
        }
        Ok(())
    }
    fn drain_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reconcile_editing_generation();
        let active = self.visible && self.page_focus.is_focused(window);
        self.shared.editing_active.store(active, Ordering::Release);
        if !active && self.visible && self.address_context.as_ref().is_some_and(|menu| menu.focus.is_focused(window) && self.focused_frame().is_some_and(|(_, owner)| owner == menu.owner)) {
            return;
        }
        if !active {
            let had_menu = self.page_context.is_some() || self.address_context.is_some();
            if had_menu { self.invalidate_editing(); self.refresh_chrome(cx); }
            return;
        }
        let Some((_, owner)) = self.focused_frame() else { self.invalidate_editing(); return; };
        let menu = self.shared.page_context.lock().ok().and_then(|mut menu| menu.take());
        if let Some(menu) = menu {
            if menu.owner == owner && menu.generation == self.editing_generation {
                self.page_context = Some(menu);
                self.refresh_chrome(cx);
            }
        }
        if self.page_context.as_ref().is_some_and(|menu| menu.owner != owner) {
            self.dismiss_context(cx);
        }

    }
}

#[cfg(test)]
mod menu_owner_tests {
    use super::{BrowserPane, BrowserOptions};
    use gpui::{prelude::*, Context, Window, TestAppContext, div};
    use gpui_shell::ShellRuntime;
    use std::{path::PathBuf, sync::{Arc, atomic::Ordering}};
    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement { div() }
    }
    fn menu(generation: u64) -> super::super::context_menu::PageContext {
        super::super::context_menu::PageContext {
            owner: super::super::context_menu::Identity { browser: 7, frame: "frame-a".into() },
            generation, token: super::super::context_menu::next_token(), link: "https://safe.test".into(),
            selection: String::new(), editable: false, can_copy: false, can_cut: false, can_paste: false,
        }
    }
    #[gpui::test]
    fn pane_revokes_menu_before_tab_swap_and_on_hide_navigation_chrome_change_and_close(cx: &mut TestAppContext) {
        cx.update(gpui_shell::init);
        let runtime = ShellRuntime::new_isolated_with_components(crate::browser_chrome::components()).unwrap();
        let window = cx.add_window(|_, _| Empty);
        window.update(cx, |_, window, cx| {
            let view = cx.new(|cx| BrowserPane::new(BrowserOptions {
                pane: "menu-owner-test".into(), workspace: "fixture".into(), url: "https://a.test".into(),
                tabs: Default::default(), profile_root: PathBuf::new(), remote: true,
            }, runtime, window, cx));
            view.update(cx, |view, cx| {
                let old = view.shared.clone();
                view.page_context = Some(menu(view.editing_generation));
                *old.page_context.lock().unwrap() = view.page_context.clone();
                let mut tabs = view.tabs.clone();
                tabs.insert("b".into(), "https://b.test".into());
                view.sync_tabs(tabs, "https://b.test", window, cx);
                assert!(!Arc::ptr_eq(&old, &view.shared));
                assert!(old.page_context.lock().unwrap().is_none());
                assert!(old.editing_generation.load(Ordering::Acquire) > 0);
                assert!(crate::browser_chrome::state("menu-owner-test")["context"].is_null());
                assert_eq!(crate::browser_chrome::state("menu-owner-test")["can_open_link_in_tab"], true);
                assert_eq!(crate::browser_chrome::state("menu-owner-test")["clipboard_available"], false);
                view.options.remote = false; // Never yield to the native creation timer.
                for action in 0..5 {
                    view.set_visible(true, window, cx);
                    view.page_context = Some(menu(view.editing_generation));
                    *view.shared.page_context.lock().unwrap() = view.page_context.clone();
                    let before = view.shared.editing_generation.load(Ordering::Acquire);
                    match action {
                        0 => view.set_visible(false, window, cx),
                        1 => view.navigate("https://changed.test".into(), window, cx),
                        2 => view.set_chrome(None, 42, cx),
                        3 => view.focus_address(window, cx),
                        _ => view.close(window, cx),
                    }
                    assert!(view.shared.page_context.lock().unwrap().is_none());
                    assert!(view.shared.editing_generation.load(Ordering::Acquire) > before);
                    assert!(crate::browser_chrome::state("menu-owner-test")["context"].is_null());
                }
            });
        }).unwrap();
        cx.run_until_parked();
    }
}
