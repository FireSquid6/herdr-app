//! Experimental Linux Wayland CPU/offscreen browser. Sandbox settings remain on;
//! this module is not a claim of a completed runtime sandbox/performance audit.
mod arguments;
mod bridge;
mod pane;
mod policy;
mod surface;
use cef::{ImplBrowser, ImplBrowserHost};
pub use pane::{BrowserEvent, BrowserOptions, BrowserPane};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct Page {
    browser: cef::Browser,
    shared: Arc<bridge::Callbacks>,
}
struct WorkspaceContext {
    context: cef::RequestContext,
    ready: Arc<AtomicBool>,
    created: Instant,
}
struct State {
    _app: Option<cef::App>,
    resources: PathBuf,
    subprocess: PathBuf,
    engine_initialized: bool,
    engine_failure: Option<String>,
    pump_count: u64,
    last_readiness_warning: Instant,
    ready: Arc<bridge::Callbacks>,
    contexts: HashMap<PathBuf, WorkspaceContext>,
    pages: Vec<Page>,
    root: PathBuf,
}
thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }
static REGISTERED: AtomicBool = AtomicBool::new(false);

fn safe_environment() -> anyhow::Result<()> {
    anyhow::ensure!(
        !std::fs::read_to_string("/proc/self/status")?
            .lines()
            .any(|l| l.starts_with("Uid:\t0\t")),
        "CEF must not run as root"
    );
    anyhow::ensure!(
        !std::env::var_os("CHROME_DEVEL_SANDBOX").is_some_and(|v| v.is_empty()),
        "empty CHROME_DEVEL_SANDBOX disables sandbox"
    );
    Ok(())
}
/// Receives full argv including argv[0], as does the app CLI. Invalid sandbox flags
/// are rejected even without a helper type, before normal CLI handling.
pub fn execute_helper(args: &[String]) -> Option<i32> {
    let invocation = match arguments::classify_argv(args) {
        Ok(v) => v,
        Err(error) => {
            eprintln!("browser helper rejected: {error}");
            return Some(2);
        }
    };
    if !matches!(invocation, arguments::Invocation::Helper) {
        return None;
    }
    if let Err(error) = safe_environment() {
        eprintln!("{error}");
        return Some(2);
    }
    let mut app = bridge::CefApplication::new(Arc::new(bridge::Callbacks::default()));
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let args = cef::args::Args::new();
    let exit = cef::execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    Some(if exit >= 0 { exit } else { 2 })
}

pub struct Runtime {
    active: bool,
    // CEF initialization, pumping and shutdown must stay on the same thread.
    _foreground: std::marker::PhantomData<std::rc::Rc<()>>,
}
/// All pane profile roots must be under HERDR_BROWSER_PROFILE_ROOT. When unset,
/// use the isolated app home/browser-profiles (the same root supplied by the UI).
/// Registers configuration only. CEF is initialized lazily for the first local
/// page, never for an exclusively remote/no-pane session. Once started, CEF has
/// process-global services: this is not a full per-workspace engine-egress audit.
pub fn initialize() -> anyhow::Result<Runtime> {
    safe_environment()?;
    anyhow::ensure!(
        std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var_os("WAYLAND_SOCKET").is_some(),
        "browser requires native Wayland"
    );
    anyhow::ensure!(
        !REGISTERED.swap(true, Ordering::AcqRel),
        "browser runtime may be registered only once per process"
    );
    let root = std::env::var_os("HERDR_BROWSER_PROFILE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::mods::app_dir().join("browser-profiles"));
    std::fs::create_dir_all(&root)?;
    let root = root.canonicalize()?;
    let resources = std::env::var_os("HERDR_BROWSER_RUNTIME")
        .or_else(|| std::env::var_os("CEF_PATH"))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("set HERDR_BROWSER_RUNTIME to the staged CEF resources"))?
        .canonicalize()?;
    anyhow::ensure!(
        std::fs::symlink_metadata(resources.join("chrome-sandbox")).is_err(),
        "CEF execution stage must omit the downloaded sibling chrome-sandbox; use the installed root-owned helper"
    );
    let subprocess = std::env::current_exe()?;
    let ready = Arc::new(bridge::Callbacks::default());
    STATE.with_borrow_mut(|state| {
        *state = Some(State {
            _app: None,
            resources,
            subprocess,
            engine_initialized: false,
            engine_failure: None,
            pump_count: 0,
            last_readiness_warning: Instant::now(),
            ready,
            contexts: HashMap::new(),
            pages: Vec::new(),
            root,
        })
    });
    Ok(Runtime {
        active: true,
        _foreground: std::marker::PhantomData,
    })
}
impl State {
    /// Called only after the remote gate. No CEF API/object construction runs at
    /// registration or during remote-only/no-pane startup.
    fn ensure_initialized(&mut self) -> anyhow::Result<()> {
        if self.engine_initialized {
            return Ok(());
        }
        if let Some(error) = &self.engine_failure {
            anyhow::bail!("{error}");
        }
        safe_environment()?;
        tracing::info!(resources = %self.resources.display(), subprocess = %self.subprocess.display(), "initializing native browser engine for first local pane");
        let mut app = bridge::CefApplication::new(self.ready.clone());
        let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
        let args = cef::args::Args::new();
        let settings = cef::Settings {
            no_sandbox: 0,
            windowless_rendering_enabled: 1,
            external_message_pump: 1,
            root_cache_path: self.root.to_string_lossy().as_ref().into(),
            browser_subprocess_path: self.subprocess.to_string_lossy().as_ref().into(),
            resources_dir_path: self.resources.to_string_lossy().as_ref().into(),
            locales_dir_path: self
                .resources
                .join("locales")
                .to_string_lossy()
                .as_ref()
                .into(),
            ..Default::default()
        };
        if cef::initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            std::ptr::null_mut(),
        ) != 1
        {
            let error = "CEF initialization failed; verify the staged runtime and installed sandbox helper. Sandbox remains required.";
            tracing::error!("{error}");
            self.engine_failure = Some(error.into());
            anyhow::bail!(error);
        }
        self._app = Some(app);
        self.engine_initialized = true;
        tracing::info!(
            ready = self.ready.ready.load(Ordering::Acquire),
            "CEF initialize returned successfully"
        );
        Ok(())
    }
}

fn pump() {
    STATE.with_borrow_mut(|state| {
        if let Some(state) = state {
            if !state.engine_initialized {
                return;
            }
            state.pump_count += 1;
            if state.pump_count == 1 {
                tracing::info!("native browser foreground pump started");
            }
            cef::do_message_loop_work();
            if !state.ready.ready.load(Ordering::Acquire)
                && state.last_readiness_warning.elapsed() >= Duration::from_secs(5)
            {
                tracing::warn!(
                    pumps = state.pump_count,
                    "CEF still waiting for on_context_initialized; page creation deferred"
                );
                state.last_readiness_warning = Instant::now();
            }
            state
                .pages
                .retain(|p| !p.shared.closed.load(Ordering::Acquire));
        }
    });
}
pub fn install(cx: &mut gpui::App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
            cx.update(|_| pump());
        }
    })
    .detach();
}
impl Runtime {
    pub fn shutdown(&mut self) {
        if !self.active {
            return;
        }
        STATE.with_borrow(|state| {
            if let Some(state) = state {
                for page in &state.pages {
                    page.shared.mailbox.close();
                    if let Some(host) = page.browser.host() {
                        host.close_browser(1);
                    }
                }
            }
        });
        let started = Instant::now();
        loop {
            pump();
            if STATE.with_borrow(|s| s.as_ref().is_none_or(|s| s.pages.is_empty())) {
                break;
            }
            if started.elapsed() > Duration::from_secs(10) {
                // Never invoke unsafe CEF shutdown while browsers remain alive.
                tracing::error!("CEF close timed out; withholding shutdown");
                self.active = false;
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let initialized = STATE
            .with_borrow_mut(|state| state.take().is_some_and(|state| state.engine_initialized));
        if initialized {
            cef::shutdown();
            tracing::info!("native browser engine shut down cleanly");
        }
        self.active = false;
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn create_page(
    options: &BrowserOptions,
    shared: Arc<bridge::Callbacks>,
    url: &str,
) -> anyhow::Result<Option<cef::Browser>> {
    anyhow::ensure!(
        policy::local_allowed(options.remote),
        "Remote browser route unavailable. Open this pane in a local workspace or configure a verified remote route."
    );
    STATE.with_borrow_mut(|state| {
        let state = state
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("browser runtime not initialized"))?;
        std::fs::create_dir_all(&options.profile_root)?;
        let root = options.profile_root.canonicalize()?;
        anyhow::ensure!(
            root.starts_with(&state.root),
            "profile_root must be inside {}",
            state.root.display()
        );
        state.ensure_initialized()?;
        // Yield to the foreground CEF loop at least once after lazy startup;
        // successful CefInitialize alone is not the custom context's readiness.
        if !state.ready.ready.load(Ordering::Acquire) || state.pump_count == 0 {
            return Ok(None);
        }
        let path = policy::profile_path(&root, &options.workspace);
        if !state.contexts.contains_key(&path) {
            std::fs::create_dir_all(&path)?;
            anyhow::ensure!(
                path.canonicalize()? == path,
                "workspace profile must not be a symlink"
            );
            let settings = cef::RequestContextSettings {
                cache_path: path.to_string_lossy().as_ref().into(),
                persist_session_cookies: 1,
                ..Default::default()
            };
            tracing::info!(workspace = %options.workspace, "creating persistent local workspace request context");
            let ready = Arc::new(AtomicBool::new(false));
            let mut handler = bridge::WorkspaceContext::new(ready.clone());
            let context = cef::request_context_create_context(Some(&settings), Some(&mut handler))
                .ok_or_else(|| anyhow::anyhow!("cannot create isolated workspace context"))?;
            state.contexts.insert(path.clone(), WorkspaceContext { context, ready, created: Instant::now() });
            // Initialization posts work to CEF. Do not attempt CreateBrowserSync
            // in the same tick, even if a callback happened synchronously.
            return Ok(None);
        }
        let workspace = state.contexts.get_mut(&path).unwrap();
        if !workspace.ready.load(Ordering::Acquire) {
            anyhow::ensure!(workspace.created.elapsed() < Duration::from_secs(20), "Workspace browser context initialization timed out. Check the staged CEF runtime/profile permissions and reopen this pane. Global context fallback is disabled.");
            return Ok(None);
        }
        let ui_thread = cef::currently_on(cef::ThreadId::UI);
        tracing::info!(workspace = %options.workspace, pane = %options.pane, origin = %policy::diagnostic_origin(url), ui_thread, "creating windowless CEF page with initialized workspace context");
        anyhow::ensure!(ui_thread == 1, "CEF page creation requires its initialization/UI thread");
        let browser = bridge::create(shared.clone(), url, &mut workspace.context)
            .ok_or_else(|| anyhow::anyhow!("CEF page creation failed"))?;
        tracing::info!(pane = %options.pane, "windowless CEF page created");
        state.pages.push(Page {
            browser: browser.clone(),
            shared,
        });
        Ok(Some(browser))
    })
}
