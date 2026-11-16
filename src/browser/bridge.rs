use super::surface::{Geometry, Mailbox, frame_len};
use cef::{rc::Rc as _, *};

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Clone, Default, PartialEq, Eq)]
pub struct LoadStatus {
    pub title: String,
    pub favicon: super::favicon::FaviconState,
    pub navigation_epoch: u64,
    pub loading: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub error: Option<String>,
    pub failed: bool,
}

#[derive(Default)]
pub struct Callbacks {
    pub mailbox: Mailbox,
    pub geometry: Mutex<Geometry>,
    pub ready: AtomicBool,
    pub closed: AtomicBool,
    pub closing: AtomicBool,
    pub navigated: Mutex<Option<String>>,
    pub status: Mutex<LoadStatus>,
    pub hidden: AtomicBool,
    pub paint_seen: AtomicBool,
    pub editing_active: AtomicBool,
    pub page_context: Mutex<Option<super::context_menu::PageContext>>,
    pub editing_generation: AtomicU64,
}

impl Callbacks {
    /// Revoke before hiding, navigating, closing or replacing the native owner.
    pub fn invalidate_editing(&self) {
        self.editing_active.store(false, Ordering::Release);
        self.editing_generation.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut menu) = self.page_context.lock() { menu.take(); }
    }
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
            self.shared.invalidate_editing();
            self.shared.closed.store(true, Ordering::Release);
        }
    }
}
wrap_load_handler! {
    pub struct Navigation { shared: Arc<Callbacks> }
    impl LoadHandler {
        fn on_loading_state_change(&self, _browser: Option<&mut Browser>, loading: i32, back: i32, forward: i32) {
            tracing::info!(loading = loading != 0, "CEF loading state changed");
            if let Ok(mut status) = self.shared.status.lock() {
                status.loading = loading != 0;
                status.can_back = back != 0;
                status.can_forward = forward != 0;
                if loading != 0 { status.error = None; status.failed = false; }
            }
        }
        fn on_load_start(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition: TransitionType) {
            if !frame.is_some_and(|frame|frame.is_main() == 1) { return; }
            if let Ok(mut status) = self.shared.status.lock() {
                status.navigation_epoch += 1;
                retire_favicon(status.favicon.document_changed());
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
// Icon requests use this browser's host/RequestContext, never GPUI HTTP or a
// global downloader. Only decoded, bounded PNG bytes become a local mod asset.
fn retire_favicon(asset: Option<String>) {
    if let Some(path) = asset { let _ = std::fs::remove_file(crate::mods::app_dir().join(path)); }
}
fn favicon_png(code: i32, image: Option<&mut Image>) -> Option<Vec<u8>> {
    if !(200..300).contains(&code) { tracing::warn!(code, "favicon download returned non-success HTTP status"); return None; }
    let Some(image) = image else { tracing::warn!(code, "favicon download returned no decoded image"); return None };
    let (mut width, mut height) = (0, 0);
    let Some(png) = image.as_png(1., 1, Some(&mut width), Some(&mut height)) else { tracing::warn!("favicon image could not encode a PNG representation"); return None };
    tracing::info!(width, height, bytes = png.size(), "favicon decoded PNG representation");
    if width <= 0 || height <= 0 || width > 64 || height > 64 || png.size() == 0 || png.size() > 65536 { return None; }
    let mut bytes = vec![0; png.size()];
    (png.data(Some(&mut bytes), 0) == bytes.len()).then_some(bytes)
}
wrap_download_image_callback! {
    pub struct Favicon { shared: Arc<Callbacks>, request: super::favicon::FaviconRequest }
    impl DownloadImageCallback {
        fn on_download_image_finished(&self, _url: Option<&CefString>, code: i32, image: Option<&mut Image>) {
            if self.shared.closing.load(Ordering::Acquire) || self.shared.closed.load(Ordering::Acquire) { return; }
            let Ok(mut status) = self.shared.status.lock() else { return };
            if !status.favicon.accepts(&self.request) { tracing::info!(code, "discarding stale favicon download reply"); return; }
            tracing::info!(code, image = image.is_some(), "current favicon download completed");
            let asset = favicon_png(code, image).and_then(|bytes| {
                use std::hash::{Hash, Hasher};
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                bytes.hash(&mut hash);
                let path = format!("browser-favicons/{}-{:x}-{:x}.png", std::process::id(), Arc::as_ptr(&self.shared) as usize, hash.finish());
                let root = crate::mods::app_dir();
                if let Err(error) = std::fs::create_dir_all(root.join("browser-favicons")).and_then(|_|std::fs::write(root.join(&path), bytes)) {
                    tracing::warn!(%error, "cannot write decoded native favicon asset"); return None;
                }
                tracing::info!(asset = %path, "native favicon asset installed");
                Some(path)
            });
            // A current failed/decode-rejected download explicitly clears its
            // icon. Old document/request replies never reach the asset writer.
            if let Ok(retired) = status.favicon.complete(&self.request, asset.clone()) {
                retire_favicon(retired.filter(|old|Some(old) != asset.as_ref()));
            }
        }
    }
}
wrap_display_handler! {
    pub struct Display { shared: Arc<Callbacks> }
    impl DisplayHandler {
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            if let Ok(mut status) = self.shared.status.lock() {
                status.title = title.map(|t|t.to_string().chars().take(512).collect()).unwrap_or_default();
            }
        }
        fn on_favicon_urlchange(&self, browser: Option<&mut Browser>, urls: Option<&mut CefStringList>) {
            if self.shared.closing.load(Ordering::Acquire) || self.shared.closed.load(Ordering::Acquire) { return; }
            // cef-rs 156's collection clone is read-only, but IntoIterator
            // asks for a mutable handle and silently yields zero for clones.
            // Move the callback's borrowed wrapper instead (not the CEF list's
            // ownership); its replacement owns only a fresh empty wrapper.
            let url = urls.and_then(|urls|std::mem::take(urls).into_iter().find(|url|url.starts_with("http://") || url.starts_with("https://")));
            tracing::info!(browser_id = browser.as_ref().map(|b|b.identifier()), candidate = url.is_some(), "CEF favicon URL update");
            let request = if let Ok(mut status) = self.shared.status.lock() {
                let (request, retired) = status.favicon.request(url);
                retire_favicon(retired);
                request
            } else { return };
            let Some(request) = request else { return };
            if let Some(host) = browser.and_then(|b|b.host()) {
                let url = request.url.clone();
                let mut callback = Favicon::new(self.shared.clone(), request);
                tracing::info!(origin = %super::policy::diagnostic_origin(&url), "requesting favicon through owning CEF browser");
                host.download_image(Some(&url.as_str().into()), 1, 32, 0, Some(&mut callback));
            }
        }
    }
}
impl Drop for Callbacks {
    fn drop(&mut self) {
        if let Ok(status) = self.status.lock() {
            if let Some(path) = &status.favicon.asset { let _ = std::fs::remove_file(crate::mods::app_dir().join(path)); }
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
    pub struct Requests { shared: Arc<Callbacks> }
    impl RequestHandler {
        fn on_before_browse(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, request: Option<&mut Request>, _gesture: i32, _redirect: i32) -> i32 {
            let url = request.map(|r| CefString::from(&r.url()).to_string()).unwrap_or_default();
            let allowed = super::policy::allowed_navigation(&url);
            if allowed { self.shared.invalidate_editing(); }
            tracing::info!(allowed, origin = %super::policy::diagnostic_origin(&url), "CEF before-browse policy");
            i32::from(!allowed)
        }
        fn resource_request_handler(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _request: Option<&mut Request>, _navigation: i32, _download: i32, _initiator: Option<&CefString>, _disable: Option<&mut i32>) -> Option<ResourceRequestHandler> { Some(Resources::new()) }
    }
}
// Both full context menus and Chromium's small selection/quick menu are
// suppressed. Only trusted chrome can render actions from this plain snapshot.
wrap_context_menu_handler! {
    pub struct ContextMenus { shared: Arc<Callbacks> }
    impl ContextMenuHandler {
        fn on_before_context_menu(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, params: Option<&mut ContextMenuParams>, model: Option<&mut MenuModel>) {
            if let Some(model) = model { model.clear(); }
            if !self.shared.editing_active.load(Ordering::Acquire) || self.shared.hidden.load(Ordering::Acquire)
                || self.shared.closed.load(Ordering::Acquire) || self.shared.closing.load(Ordering::Acquire) { return; }
            let (Some(browser), Some(frame), Some(params)) = (browser, frame, params) else { return; };
            let link = CefString::from(&params.link_url()).to_string();
            let selection = CefString::from(&params.selection_text()).to_string();
            let flags = params.edit_state_flags().as_ref().0;
            if let Ok(mut context) = self.shared.page_context.lock() {
                *context = super::context_menu::PageContext::snapshot(
                    super::context_menu::identity(browser, frame),
                    self.shared.editing_generation.load(Ordering::Acquire),
                    link, selection, params.is_editable() == 1, flags,
                );
            }
        }
        fn run_context_menu(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _params: Option<&mut ContextMenuParams>, _model: Option<&mut MenuModel>, callback: Option<&mut RunContextMenuCallback>) -> i32 {
            if let Some(callback) = callback { callback.cancel(); }
            1
        }
        fn run_quick_menu(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _location: Option<&Point>, _size: Option<&Size>, _flags: QuickMenuEditStateFlags, callback: Option<&mut RunQuickMenuCallback>) -> i32 {
            if let Some(callback) = callback { callback.cancel(); }
            1
        }
    }
}
wrap_frame_handler! {
    pub struct Frames { shared: Arc<Callbacks> }
    impl FrameHandler {
        fn on_frame_detached(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>) { self.shared.invalidate_editing(); }
        fn on_frame_destroyed(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>) { self.shared.invalidate_editing(); }
        fn on_main_frame_changed(&self, _browser: Option<&mut Browser>, _old: Option<&mut Frame>, _new: Option<&mut Frame>) { self.shared.invalidate_editing(); }
    }
}
wrap_client! {
    pub struct PageClient { display: DisplayHandler, pixels: RenderHandler, lifetime: LifeSpanHandler, navigation: LoadHandler, permissions: PermissionHandler, dialogs: DialogHandler, downloads: DownloadHandler, requests: RequestHandler, menus: ContextMenuHandler, frames: FrameHandler }
    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> { Some(self.display.clone()) }
        fn render_handler(&self) -> Option<RenderHandler> { Some(self.pixels.clone()) }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(self.lifetime.clone()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(self.navigation.clone()) }
        fn dialog_handler(&self) -> Option<DialogHandler> { Some(self.dialogs.clone()) }
        fn download_handler(&self) -> Option<DownloadHandler> { Some(self.downloads.clone()) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(self.requests.clone()) }
        fn permission_handler(&self) -> Option<PermissionHandler> { Some(self.permissions.clone()) }
        fn frame_handler(&self) -> Option<FrameHandler> { Some(self.frames.clone()) }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> { Some(self.menus.clone()) }
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
        Display::new(shared.clone()),
        Pixels::new(shared.clone()),
        Lifetime::new(shared.clone()),
        Navigation::new(shared.clone()),
        Permissions::new(),
        Dialogs::new(),
        Downloads::new(),
        Requests::new(shared.clone()),
        ContextMenus::new(shared.clone()),
        Frames::new(shared),
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

#[cfg(test)]
mod tab_visibility_tests {
    use super::*;
    #[test]
    fn favicon_callback_reads_the_original_cef_list_not_a_readonly_clone() {
        let shared = Arc::new(Callbacks::default());
        let display = Display::new(shared.clone());
        let mut urls = CefStringList::new();
        urls.append("http://127.0.0.1/owned/favicon.png");
        display.on_favicon_urlchange(None, Some(&mut urls));
        assert_eq!(shared.status.lock().unwrap().favicon.pending_url(), Some("http://127.0.0.1/owned/favicon.png"));
        let mut empty = CefStringList::new();
        display.on_favicon_urlchange(None, Some(&mut empty));
        assert!(shared.status.lock().unwrap().favicon.pending_url().is_none());
    }
    #[test]
    fn cef_hidden_paint_callback_never_copies_frames() {
        let shared = Arc::new(Callbacks::default());
        let pixels = Pixels::new(shared.clone());
        let bytes = vec![0u8; 16];
        pixels.on_paint(None, PaintElementType::VIEW, None, bytes.as_ptr().cast(), 2, 2);
        assert_eq!(shared.mailbox.counts().0, 1);
        shared.hidden.store(true, Ordering::Release);
        for _ in 0..5 { pixels.on_paint(None, PaintElementType::VIEW, None, bytes.as_ptr().cast(), 2, 2); }
        assert_eq!(shared.mailbox.counts().0, 1);
        shared.hidden.store(false, Ordering::Release);
        shared.mailbox.close();
        pixels.on_paint(None, PaintElementType::VIEW, None, bytes.as_ptr().cast(), 2, 2);
        assert_eq!(shared.mailbox.counts().0, 1);
    }
}

#[cfg(test)]
mod context_menu_tests {
    use super::*;
    #[test]
    fn owner_invalidation_retires_queued_native_metadata_and_generation() {
        let shared = Callbacks::default();
        shared.editing_active.store(true, Ordering::Release);
        *shared.page_context.lock().unwrap() = Some(super::super::context_menu::PageContext {
            owner: super::super::context_menu::Identity {browser: 7, frame: "a".into()},
            generation: 0, token: 1, link: "https://safe.test".into(), selection: String::new(),
            editable: false, can_copy: false, can_cut: false, can_paste: false,
        });
        shared.invalidate_editing();
        assert!(!shared.editing_active.load(Ordering::Acquire));
        assert_eq!(shared.editing_generation.load(Ordering::Acquire), 1);
        assert!(shared.page_context.lock().unwrap().is_none());
    }
    #[test]
    fn native_close_invalidates_menus_immediately_not_at_next_render() {
        let shared = Arc::new(Callbacks::default());
        shared.editing_active.store(true, Ordering::Release);
        Lifetime::new(shared.clone()).on_before_close(None);
        assert!(shared.closed.load(Ordering::Acquire));
        assert!(!shared.editing_active.load(Ordering::Acquire));
        assert_eq!(shared.editing_generation.load(Ordering::Acquire), 1);
    }
}

#[cfg(test)]
mod native_menu_suppression_tests {
    use super::*;
    use cef::rc::{ConvertReturnValue, RcImpl};
    use cef::sys::{_cef_run_context_menu_callback_t as Full, _cef_run_quick_menu_callback_t as Quick};
    extern "C" fn cancel_full(raw: *mut Full) {
        RcImpl::<Full, Arc<AtomicBool>>::get(raw).interface.store(true, Ordering::Release);
    }
    extern "C" fn cancel_quick(raw: *mut Quick) {
        RcImpl::<Quick, Arc<AtomicBool>>::get(raw).interface.store(true, Ordering::Release);
    }
    fn full_callback(cancelled: Arc<AtomicBool>) -> RunContextMenuCallback {
        // SAFETY: this C callback struct contains only scalar fields and nullable
        // function pointers. RcImpl installs valid refcount callbacks before use.
        let mut raw: Full = unsafe { std::mem::zeroed() };
        raw.cancel = Some(cancel_full);
        let raw = RcImpl::new(raw, cancelled);
        (raw.cast::<Full>()).wrap_result()
    }
    fn quick_callback(cancelled: Arc<AtomicBool>) -> RunQuickMenuCallback {
        // SAFETY: same scalar/nullable C layout and RcImpl ownership as above.
        let mut raw: Quick = unsafe { std::mem::zeroed() };
        raw.cancel = Some(cancel_quick);
        let raw = RcImpl::new(raw, cancelled);
        (raw.cast::<Quick>()).wrap_result()
    }
    #[test]
    fn native_full_and_quick_menus_are_handled_and_cancelled_even_without_owner() {
        let menus = ContextMenus::new(Arc::new(Callbacks::default()));
        let full = Arc::new(AtomicBool::new(false));
        let quick = Arc::new(AtomicBool::new(false));
        assert_eq!(menus.run_context_menu(None, None, None, None, Some(&mut full_callback(full.clone()))), 1);
        assert_eq!(menus.run_quick_menu(None, None, None, None, QuickMenuEditStateFlags::default(), Some(&mut quick_callback(quick.clone()))), 1);
        assert!(full.load(Ordering::Acquire));
        assert!(quick.load(Ordering::Acquire));
    }
    #[test]
    fn frame_replacement_detach_and_destruction_revoke_before_next_tick() {
        let shared = Arc::new(Callbacks::default());
        let frames = Frames::new(shared.clone());
        for action in 0..3 {
            shared.editing_active.store(true, Ordering::Release);
            match action {
                0 => frames.on_main_frame_changed(None, None, None),
                1 => frames.on_frame_detached(None, None),
                _ => frames.on_frame_destroyed(None, None),
            }
            assert!(!shared.editing_active.load(Ordering::Acquire));
            assert_eq!(shared.editing_generation.load(Ordering::Acquire), action + 1);
        }
    }
}
