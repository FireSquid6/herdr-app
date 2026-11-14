use super::{bridge::Callbacks, surface::Geometry};
use cef::{ImplBrowser, ImplBrowserHost, ImplFrame};
use gpui::{prelude::*, *};
use gpui_base::input::{Input, InputEvent, InputState};
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
    pub profile_root: PathBuf,
    pub remote: bool,
}
pub enum BrowserEvent {
    Navigated(String),
}
impl EventEmitter<BrowserEvent> for BrowserPane {}
#[derive(Default)]
struct Surface {
    image: Option<Arc<RenderImage>>,
    bounds: Bounds<Pixels>,
}
pub struct BrowserPane {
    options: BrowserOptions,
    surface: Rc<RefCell<Surface>>,
    shared: Arc<Callbacks>,
    browser: Option<cef::Browser>,
    focus: FocusHandle,
    page_focus: FocusHandle,
    closed: bool,
    visible: bool,
    status: super::bridge::LoadStatus,
    address: Entity<InputState>,
    geometry: Geometry,
    url: String,
    error: Option<String>,
}
impl BrowserPane {
    pub fn new(options: BrowserOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.on_release_in(window, |this, window, _| {
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
        let url = crate::settings::browser_target(&options.url);
        let address = cx.new(|cx| {
            let mut state =
                InputState::new(window, cx).placeholder("Search Google or enter address");
            state.set_value(url.clone(), window, cx);
            state
        });
        cx.subscribe_in(
            &address,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Focus) {
                    if let Some(host) = this.host() {
                        host.set_focus(0);
                    }
                }
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let url = input.read(cx).value().to_string();
                    this.navigate(url, window, cx);
                }
            },
        )
        .detach();
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
        Self {
            options,
            surface: Rc::new(RefCell::new(Surface::default())),
            shared: Arc::new(Callbacks::default()),
            browser: None,
            focus,
            page_focus,
            closed: false,
            visible: true,
            status: super::bridge::LoadStatus::default(),
            address,
            geometry: Geometry::default(),
            url,
            error,
        }
    }
    /// Hidden pages retain their last atlas image and process lifecycle/navigation
    /// events, but do not create browsers, accept paint copies or upload images.
    pub fn set_visible(&mut self, visible: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.visible == visible {
            return;
        }
        tracing::info!(pane = %self.options.pane, visible, "native browser pane visibility changed");
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
        cx.notify();
    }
    /// Explicit ownership teardown: frame references may keep this entity alive.
    /// CEF's runtime registry retains the browser only until OnBeforeClose.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_close();
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
        self.shared.hidden.store(true, Ordering::Release);
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
    pub fn navigate(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.options.remote {
            return;
        }
        self.url = crate::settings::browser_target(&url);
        self.address.update(cx, |state, cx| {
            state.set_value(self.url.clone(), window, cx)
        });
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
    pub fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed {
            return;
        }
        if let Some(host) = self.host() {
            host.set_focus(0);
        }
        self.address.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
    }
    fn host(&self) -> Option<cef::BrowserHost> {
        if self.closed {
            return None;
        }
        self.browser.as_ref()?.host()
    }
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.error.is_some() {
            return;
        }
        if self.visible && self.browser.is_none() {
            match super::create_page(&self.options, self.shared.clone(), &self.url) {
                Ok(browser) => {
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
            self.address
                .update(cx, |state, cx| state.set_value(url.clone(), window, cx));
            cx.emit(BrowserEvent::Navigated(url));
        }
        let status = self.shared.status.lock().ok().map(|status| status.clone());
        if let Some(status) = status {
            if status != self.status {
                self.status = status;
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
                    cx.notify();
                }
            }
        }
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
impl Render for BrowserPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pre = self.surface.clone();
        let paint = self.surface.clone();
        let shared = self.shared.clone();
        let content = div()
            .id("browser")
            .flex_1()
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
        div()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .bg(rgb(0x25252b))
                    .text_color(rgb(0xeeeeee))
                    .flex_shrink_0()
                    .child(
                        div()
                            .id("back")
                            .child("Back")
                            .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                    )
                    .child(
                        div()
                            .id("forward")
                            .child("Forward")
                            .on_click(cx.listener(|this, _, window, cx| this.forward(window, cx))),
                    )
                    .child(
                        div()
                            .id("reload")
                            .child("Reload")
                            .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
                    )
                    .child(
                        div()
                            .id("stop")
                            .child("Stop")
                            .on_click(cx.listener(|this, _, window, cx| this.stop(window, cx))),
                    )
                    .child(div().flex_1().min_w_0().child(Input::new(&self.address))),
            )
            .child(div().px_2().py_1().text_sm().child(format!(
                "Workspace: {} · {}",
                self.options.workspace.chars().take(128).collect::<String>(),
                if self.options.remote {
                    "Remote route blocked"
                } else {
                    "Local networking · localhost is this computer"
                }
            )))
            .when(self.status.loading, |view| {
                view.child(div().px_2().child("Loading…"))
            })
            .when_some(self.status.error.clone(), |view, error| {
                view.child(div().p_2().child(error))
            })
            .when_some(self.error.clone(), |view, error| {
                view.child(div().p_4().child(error))
            })
            .when(!self.closed && self.error.is_none(), |view| {
                view.child(content)
            })
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
