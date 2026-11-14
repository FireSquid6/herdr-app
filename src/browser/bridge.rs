use super::surface::{Geometry, Mailbox, frame_len};
use cef::{rc::Rc as _, *};

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Default, PartialEq, Eq)]
pub struct LoadStatus {
    pub loading: bool,
    pub error: Option<String>,
    pub failed: bool,
}

#[derive(Default)]
pub struct Callbacks {
    pub mailbox: Mailbox,
    pub geometry: Mutex<Geometry>,
    pub ready: AtomicBool,
    pub closed: AtomicBool,
    pub navigated: Mutex<Option<String>>,
    pub status: Mutex<LoadStatus>,
    pub hidden: AtomicBool,
    pub paint_seen: AtomicBool,
}

wrap_render_handler! {
    pub struct Pixels { shared: Arc<Callbacks> }
    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let (Some(r), Ok(g)) = (rect, self.shared.geometry.lock()) {
                *r = Rect { x: 0, y: 0, width: g.width, height: g.height };
            }
        }
        fn screen_info(&self, _browser: Option<&mut Browser>, info: Option<&mut ScreenInfo>) -> i32 {
            if let (Some(i), Ok(g)) = (info, self.shared.geometry.lock()) {
                i.device_scale_factor = g.scale;
                i.rect = Rect { x: 0, y: 0, width: g.width, height: g.height };
                i.available_rect = i.rect.clone();
                return 1;
            }
            0
        }
        fn on_paint(&self, _browser: Option<&mut Browser>, kind: PaintElementType,
            _dirty: Option<&[Rect]>, buffer: *const u8, width: i32, height: i32) {
            if self.shared.hidden.load(Ordering::Acquire) || kind != PaintElementType::VIEW || buffer.is_null() { return; }
            let Some(len) = frame_len(width,height) else { return; };
            if !self.shared.paint_seen.swap(true, Ordering::AcqRel) { tracing::info!(width, height, "first CEF view paint received"); }
            // SAFETY: CEF owns a full width*height*4 BGRA buffer until this callback returns.
            // Dimensions are validated before constructing the callback-lifetime slice.
            let bytes = unsafe { std::slice::from_raw_parts(buffer, len) };
            self.shared.mailbox.publish(width,height,bytes);
        }
    }
}
wrap_life_span_handler! {
    pub struct Lifetime { shared: Arc<Callbacks> }
    impl LifeSpanHandler {
        fn on_before_popup(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _id: i32,
            _url: Option<&CefString>, _name: Option<&CefString>, _disposition: WindowOpenDisposition,
            _gesture: i32, _features: Option<&PopupFeatures>, _info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>,
            _extra: Option<&mut Option<DictionaryValue>>, _access: Option<&mut i32>) -> i32 { 1 }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            tracing::info!("CEF page on_before_close");
            self.shared.closed.store(true, Ordering::Release);
        }
    }
}
wrap_load_handler! {
    pub struct Navigation { shared: Arc<Callbacks> }
    impl LoadHandler {
        fn on_loading_state_change(&self, _browser: Option<&mut Browser>, loading: i32, _back: i32, _forward: i32) {
            tracing::info!(loading = loading != 0, "CEF loading state changed");
            if let Ok(mut status) = self.shared.status.lock() {
                status.loading = loading != 0;
                if loading != 0 { status.error = None; status.failed = false; }
            }
        }
        fn on_load_error(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, code: Errorcode, text: Option<&CefString>, _url: Option<&CefString>) {
            // ERR_ABORTED accompanies deliberate Stop/replacement navigation.
            if code == Errorcode::ABORTED || !frame.is_some_and(|frame| frame.is_main() == 1) { return; }
            tracing::warn!(?code, "CEF main-frame load failed; recoverable native feedback shown");
            let detail: String = text.map(|s| s.to_string()).unwrap_or_else(|| "Network request failed".into()).chars().take(512).collect();
            if let Ok(mut status) = self.shared.status.lock() {
                status.failed = true;
                status.error = Some(format!("Page could not load: {detail}. Check the address/connection, then Reload or navigate to another address."));
            }
            if let Ok(mut latest) = self.shared.navigated.lock() { latest.take(); }
        }
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status: i32) {
            if let Some(frame) = frame {
                if frame.is_main() != 1 { return; }
                let url = CefString::from(&frame.url()).to_string();
                tracing::info!(http_status, origin = %super::policy::diagnostic_origin(&url), "CEF main-frame load ended");
                if let Ok(mut status) = self.shared.status.lock() {
                    if http_status >= 400 {
                        status.failed = true;
                        status.error = Some(format!("Server returned HTTP {http_status}. Check the address, Reload, or navigate elsewhere."));
                    }
                    if !status.failed && (url.starts_with("https://") || url.starts_with("http://")) {
                        if let Ok(mut latest) = self.shared.navigated.lock() { *latest = Some(url); }
                    }
                }
            }
        }
    }
}
wrap_permission_handler! {
    pub struct Permissions {}
    impl PermissionHandler {
        fn on_request_media_access_permission(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _origin: Option<&CefString>, _requested: u32, callback: Option<&mut MediaAccessCallback>) -> i32 {
            if let Some(callback) = callback { callback.cont(0); }
            1
        }

        fn on_show_permission_prompt(&self, _browser: Option<&mut Browser>, _id: u64,
            _origin: Option<&CefString>, _requested: u32, callback: Option<&mut PermissionPromptCallback>) -> i32 {
            if let Some(callback) = callback { callback.cont(PermissionRequestResult::DENY); }
            1
        }
    }
}
// No native dialogs, filesystem uploads, downloads or external protocol launch.
wrap_dialog_handler! {
    pub struct Dialogs {}
    impl DialogHandler {
        fn on_file_dialog(&self, _browser: Option<&mut Browser>, _mode: FileDialogMode,
            _title: Option<&CefString>, _path: Option<&CefString>, _filters: Option<&mut CefStringList>,
            _extensions: Option<&mut CefStringList>, _descriptions: Option<&mut CefStringList>,
            callback: Option<&mut FileDialogCallback>) -> i32 {
            if let Some(callback) = callback { callback.cancel(); }
            1
        }
    }
}
wrap_download_handler! {
    pub struct Downloads {}
    impl DownloadHandler {
        fn can_download(&self, _browser: Option<&mut Browser>, _url: Option<&CefString>, _method: Option<&CefString>) -> i32 { 0 }
        fn on_before_download(&self, _browser: Option<&mut Browser>, _item: Option<&mut DownloadItem>, _name: Option<&CefString>, _callback: Option<&mut BeforeDownloadCallback>) -> i32 { 1 }
    }
}
wrap_resource_request_handler! {
    pub struct Resources {}
    impl ResourceRequestHandler {
        fn on_before_resource_load(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>, _callback: Option<&mut Callback>) -> ReturnValue {
            if request.is_some_and(|r| super::policy::allowed_resource(&CefString::from(&r.url()).to_string())) { ReturnValue::CONTINUE } else { ReturnValue::CANCEL }
        }
        fn on_protocol_execution(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _request: Option<&mut Request>, allow: Option<&mut i32>) { if let Some(allow) = allow { *allow = 0; } }
    }
}
wrap_request_handler! {
    pub struct Requests {}
    impl RequestHandler {
        fn on_before_browse(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>, _gesture: i32, _redirect: i32) -> i32 {
            let url = request.map(|r| CefString::from(&r.url()).to_string()).unwrap_or_default();
            let allowed = super::policy::allowed_navigation(&url);
            tracing::info!(allowed, origin = %super::policy::diagnostic_origin(&url), "CEF before-browse policy");
            i32::from(!allowed)
        }
        fn resource_request_handler(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _request: Option<&mut Request>, _navigation: i32, _download: i32, _initiator: Option<&CefString>, _disable: Option<&mut i32>) -> Option<ResourceRequestHandler> { Some(Resources::new()) }
    }
}
wrap_client! {
    pub struct PageClient { pixels: RenderHandler, lifetime: LifeSpanHandler, navigation: LoadHandler, permissions: PermissionHandler, dialogs: DialogHandler, downloads: DownloadHandler, requests: RequestHandler }
    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> { Some(self.pixels.clone()) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(self.lifetime.clone()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(self.navigation.clone()) }
        fn dialog_handler(&self) -> Option<DialogHandler> { Some(self.dialogs.clone()) }
        fn download_handler(&self) -> Option<DownloadHandler> { Some(self.downloads.clone()) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(self.requests.clone()) }
        fn permission_handler(&self) -> Option<PermissionHandler> { Some(self.permissions.clone()) }
    }
}
// CEF 156 include/cef_request_context_handler.h: this UI-thread callback,
// not the process-wide OnContextInitialized callback, establishes readiness of
// a newly created non-global context. The context owns its handler lifetime.
wrap_request_context_handler! {
    pub struct WorkspaceContext { ready: Arc<AtomicBool> }
    impl RequestContextHandler {
        fn on_request_context_initialized(&self, _context: Option<&mut RequestContext>) {
            tracing::info!(ui_thread = currently_on(ThreadId::UI), "workspace request context initialized");
            self.ready.store(true, Ordering::Release);
        }
    }
}

wrap_browser_process_handler! {
    pub struct Process { shared: Arc<Callbacks> }
    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            tracing::info!("CEF browser context initialized; page creation ready");
            self.shared.ready.store(true, Ordering::Release);
        }
    }
}
wrap_app! {
    pub struct CefApplication { shared: Arc<Callbacks> }
    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(Process::new(self.shared.clone()))
        }
        fn on_before_command_line_processing(&self, _kind: Option<&CefString>, command: Option<&mut CommandLine>) {
            if let Some(command) = command {
                command.append_switch_with_value(Some(&"ozone-platform".into()), Some(&"headless".into()));
                command.append_switch(Some(&"disable-gpu".into()));
                command.append_switch(Some(&"disable-background-networking".into()));
                command.append_switch(Some(&"no-first-run".into()));
            }
        }
    }
}

pub fn create(shared: Arc<Callbacks>, url: &str, context: &mut RequestContext) -> Option<Browser> {
    let mut client = PageClient::new(
        Pixels::new(shared.clone()),
        Lifetime::new(shared.clone()),
        Navigation::new(shared),
        Permissions::new(),
        Dialogs::new(),
        Downloads::new(),
        Requests::new(),
    );
    let info = WindowInfo::default().set_as_windowless(0);
    let settings = BrowserSettings {
        windowless_frame_rate: 30,
        background_color: 0xff202028,
        ..Default::default()
    };
    browser_host_create_browser_sync(
        Some(&info),
        Some(&mut client),
        Some(&url.into()),
        Some(&settings),
        None,
        Some(context),
    )
}

pub fn text(host: &BrowserHost, text: &str) {
    for character in text.encode_utf16() {
        host.send_key_event(Some(&KeyEvent {
            type_: KeyEventType::CHAR,
            windows_key_code: character as i32,
            character,
            unmodified_character: character,
            ..Default::default()
        }));
    }
}
