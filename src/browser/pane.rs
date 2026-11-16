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

pub struct BrowserPane {
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
            if let Some(host) = this.host() {
                host.set_focus(1);
            }
        })
        .detach();
        cx.on_blur(&page_focus, window, |this, _, _| {
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
        self.url = crate::settings::browser_target(&url);
        if let Some(tab) = self.tabs.items.iter_mut().find(|t|t.id == self.tabs.selected) { tab.url = self.url.clone(); }
        self.refresh_chrome(cx);
        crate::mods::deliver_command_to(self.chrome_copy, "navigated");
        if let Some(frame) = self.browser.as_ref().and_then(|b| b.main_frame()) {
            frame.load_url(Some(&self.url.as_str().into()));
        }
        cx.notify();
    }
    pub fn back(&mut self, _: &mut Window, _: &mut Context<Self>) {
        if let Some(b) = &self.browser {
            b.go_back();
        }
    }
    pub fn forward(&mut self, _: &mut Window, _: &mut Context<Self>) {
        if let Some(b) = &self.browser {
            b.go_forward();
        }
    }
    pub fn reload(&mut self, _: &mut Window, _: &mut Context<Self>) {
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
        if let Some(host) = self.host() {
            host.set_focus(0);
        }
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
        }), self.page_view.downgrade());
    }
    /// Chrome is an ordinary app-owned mod copy, including hot reload and close.
    pub fn set_chrome(&mut self, root: Option<Entity<ShellRoot>>, copy: u64, cx: &mut Context<Self>) {
        if self.chrome_copy == copy { return; }
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
        let navigated = self.shared.navigated.lock().ok().and_then(|mut n| n.take());
        if let Some(url) = navigated {
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, _| {
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
