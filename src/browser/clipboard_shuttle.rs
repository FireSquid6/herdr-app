//! Global, UI-thread-only CEF plaintext shuttle, driven by the browser runtime.
//!
//! Integration contract (all hooks are required):
//! - Construct ONE Coordinator after CEF initialization; retain it through close.
//! - Forward render-process browser-created/destroyed/message callbacks to ONE
//!   RendererHooks instance, including in subprocess Application construction.
//!   Only browser-created extra_info registers a helper; a URL never does.
//! - Pump CEF outside this module, then tick(Instant::now()). Callbacks touch only
//!   Arc/Mutex mailboxes, never the runtime's RefCell or this coordinator.
//! - Serialize ALL app clipboard commands globally, not once per pane/workspace.
//!   Gate availability using CURRENT native CEF edit flags (CanCopy/CanCut/
//!   CanPaste); this actor advertises no UI and queries no source DOM.
//!   Capture an OwnerStamp from native app/page/document ownership, invalidate on
//!   focus/tab/navigation/close/address-copy, and revalidate it before BOTH
//!   dispatch_source and GPUI export. Metadata/website IPC is not authorization.
//! - Ordinary pages must independently use javascript_access_clipboard and
//!   javascript_dom_paste DISABLED, CLIPBOARD_READ_WRITE BLOCK, prompts DENY.
//! - Parent snapshots GPUI plaintext for begin_paste (None != Some("")); this
//!   module never calls an OS/GPUI clipboard API. No source DOM inspection.
//! - shutdown() is nonblocking; drain outstanding_helper_count()==0 before
//!   dropping the actor's CEF references and calling cef::shutdown, even though
//!   the runtime's ordinary pages exclude the helper.
//!
//! Copy/Cut observes the shared CEF clipboard, NOT cryptographic attribution or
//! an engine commit barrier. A healthy observation window ending at the sentinel
//! is NoUpdateObserved, NOT successful/proven cancellation, and permits reuse.
//! Every subsequent transaction starts with a new verified seed. Ordinary
//! gesture writers and late clipboard changes remain normal shared-clipboard
//! races. Ownership invalidation abandons export but drains outstanding work;
//! only genuine transport/seed failure poisons until app restart. Cut is NEVER
//! retried. Paste keeps the global slot through a source-native Paste followed
//! immediately by a command-free debugger keyUp ACK on the same normally
//! delivered widget input queue. This is NOT an unconditional CDP-success
//! barrier: browser input filters, suppression, widget/lifecycle changes and
//! ACK routing remain integration guards/acceptance requirements. The app must
//! be the sole debugger keyboard injector, with globally serialized method IDs.
//! PasteDispatched follows the matching ACK, not merely method submission;
//! it still promises neither DOM insertion nor an atomic focus freeze. The
//! extra trusted keyUp is observable. Paste data is deliberately NOT scrubbed.
//!
//! This is a plaintext projection, not a rich-MIME bridge: a trusted helper
//! paste reporting NO formats after the verified nonempty Copy/Cut sentinel is
//! an observed clear at the healthy observation window's end, projected as
//! CopiedText(""). Formats present without plain
//! text remain unsupported, NOT an empty-text export. An empty Paste seed may
//! likewise verify as no formats; native source Paste may then be a no-op, and
//! PasteDispatched does not promise a text/plain offer, event or DOM insertion.
//!
//! Payloads are bounded, transient and never logged; do not Debug/log results.
//! Runtime integration and private-native tests must preserve these boundaries.

use cef::{rc::Rc as _, *};
use std::{
    collections::HashMap,
    marker::PhantomData,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

pub const MAX_TEXT_BYTES: usize = 64 * 1024;
const SETUP_LIMIT: Duration = Duration::from_secs(10);
const SEED_LIMIT: Duration = Duration::from_secs(5);
const IPC_LIMIT: Duration = Duration::from_secs(1);
const EVENT_LIMIT: Duration = Duration::from_secs(1);
const SOURCE_FENCE_LIMIT: Duration = Duration::from_secs(3);
// A bounded observation policy, NOT a sleep/commit/cancellation barrier.
const OBSERVATION_WINDOW: Duration = Duration::from_millis(500);
const PROTOCOL_VERSION: i32 = 2;
const CALL: &str = "herdr.private.clipboard-shuttle.call.v2";
const REPLY: &str = "herdr.private.clipboard-shuttle.reply.v2";
const MARKER_KEY: &str = "herdr.private.clipboard-shuttle.marker";
const VERSION_KEY: &str = "herdr.private.clipboard-shuttle.version";
static CLAIMED: AtomicBool = AtomicBool::new(false);
static IDS: AtomicU64 = AtomicU64::new(0);
static LIVE_HELPERS: AtomicUsize = AtomicUsize::new(0);
static CREATING_HELPERS: AtomicUsize = AtomicUsize::new(0);

fn next_id() -> Result<u64, Failure> {
    IDS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_add(1))
        .map(|v| v + 1)
        .map_err(|_| Failure::IdentifierExhausted)
}
fn bounded(text: &str) -> bool {
    text.len() <= MAX_TEXT_BYTES
}
fn decimal(value: &str, allow_zero: bool) -> Option<u64> {
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let id = value.parse::<u64>().ok()?;
    ((allow_zero || id != 0) && id.to_string() == value).then_some(id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransactionId(u64);
impl TransactionId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Opaque app owner and document generation. Only trusted parent code assigns
/// these, and parent MUST compare against its CURRENT owner before dispatch and
/// export; the actor cannot know native pane/tab/document lifecycle by itself.
/// Include focused SUBFRAME document changes too, not only main-frame navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnerStamp {
    pub owner: u64,
    pub document_generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Copy,
    Cut,
    Paste,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Busy,
    NotReady,
    AlreadyConstructed,
    IdentifierExhausted,
    PayloadTooLarge,
    NoPlaintext,
    OwnerChanged,
    Deadline,
    HelperUnavailable,
    HelperProtocol,
    HelperTimeout,
    SeedNotVerified,
    Cancelled,
    ShuttingDown,
    RestartRequired,
}
/// NoUpdateObserved is a healthy *non-success*, with no OS export. It is neither
/// cancellation detection nor proof that a native write cannot arrive later.
/// Empty CopiedText is valid and MUST NOT be confused with NoUpdateObserved.
/// It also projects an observed no-format CEF clear to OS plaintext empty; this
/// does NOT claim that the native clipboard offers a present text/plain format.
pub enum CompletionResult {
    CopiedText(String),
    NoUpdateObserved,
    /// Native Paste plus a correlated command-free keyUp result, not proof of
    /// insertion or an unconditional fence across widget/lifecycle exceptions.
    PasteDispatched,
    Failed(Failure),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeFrameIdentity {
    pub browser_id: i32,
    pub frame_id: String,
}
pub struct Completion {
    pub transaction: TransactionId,
    pub owner: OwnerStamp,
    pub operation: Operation,
    pub source: NativeFrameIdentity,
    pub result: CompletionResult,
    pub coordinator_poisoned: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyForSource {
    pub transaction: TransactionId,
    pub owner: OwnerStamp,
    pub operation: Operation,
    pub browser_id: i32,
    pub frame_id: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Text,
    NoUpdateObserved,
    PasteDispatched,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    SetupPending,
    Idle,
    SeedPending(TransactionId),
    ReadyForSource(ReadyForSource),
    WaitingResult(TransactionId),
    AbandonedDraining(TransactionId),
    Complete {
        transaction: TransactionId,
        kind: CompletionKind,
        poisoned: bool,
    },
    Unavailable(Failure),
    Poisoned(Failure),
    Closing,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
enum WireOp {
    Init = 0,
    ArmCopy = 1,
    ArmPaste = 2,
    Snapshot = 3,
}
impl WireOp {
    fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Init),
            1 => Some(Self::ArmCopy),
            2 => Some(Self::ArmPaste),
            3 => Some(Self::Snapshot),
            _ => None,
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
struct WireKey {
    browser: i32,
    frame: String,
    marker: String,
    transaction: u64,
    nonce: u64,
    op: WireOp,
}
struct Pending {
    key: WireKey,
    deadline: Instant,
}
struct WireReply {
    ok: bool,
    copy_seen: bool,
    paste_seen: bool,
    has_plaintext: bool,
    has_formats: bool,
    oversized: bool,
    text: String,
}
#[derive(Default)]
struct Mailbox {
    pending: Option<Pending>,
    reply: Option<WireReply>,
}
#[derive(Default)]
struct Shared {
    context_ready: AtomicBool,
    loaded: AtomicBool,
    fault: AtomicBool,
    closed: AtomicBool,
    live: AtomicBool,
    creating: AtomicBool,
    close_requested: AtomicBool,
    created_browser: Mutex<Option<i32>>,
    initial_navigation: AtomicBool,
    mailbox: Mutex<Mailbox>,
}
fn accepts(pending: &Pending, key: &WireKey, now: Instant) -> bool {
    pending.key == *key && now < pending.deadline
}

// This fence relies on NORMAL same-widget associated input-queue delivery,
// not a CEF process-message ACK (that uses a different Mojo pipe). A successful
// CDP result does not expose input ACK source/state or a unique event ID. Parent
// ownership/widget lifecycle guards and exclusive debugger injection therefore
// remain mandatory; ignored/filtered/suppressed input is not universally fenced.
const PASTE_FENCE_PARAMS: &str =
    r#"{"type":"keyUp","key":"Unidentified","code":"Unidentified","windowsVirtualKeyCode":0}"#;
fn checked_fence_method_id(value: u64) -> Option<i32> {
    i32::try_from(value).ok().filter(|id| *id > 0)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FenceKey {
    browser_id: i32,
    method_id: i32,
}
struct FenceExpectation {
    key: FenceKey,
    deadline: Instant,
}
struct FenceMailbox {
    expected: Option<FenceExpectation>,
    result: Option<bool>,
    detached: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FencePoll {
    Pending,
    Acknowledged,
    Failed(Failure),
}
impl FenceMailbox {
    fn new(key: FenceKey, now: Instant, deadline: Instant) -> Result<Self, Failure> {
        if key.browser_id <= 0 || key.method_id <= 0 || deadline <= now {
            return Err(Failure::HelperProtocol);
        }
        Ok(Self {
            expected: Some(FenceExpectation { key, deadline }),
            result: None,
            detached: false,
        })
    }
    fn record_result(&mut self, key: FenceKey, success: bool, arrived: Instant) -> bool {
        let Some(expected) = &self.expected else {
            return false;
        };
        if key != expected.key
            || arrived >= expected.deadline
            || self.detached
            || self.result.is_some()
        {
            return false;
        }
        self.result = Some(success);
        true
    }
    fn record_detached(&mut self, browser_id: i32) {
        if self
            .expected
            .as_ref()
            .is_some_and(|expected| expected.key.browser_id == browser_id)
        {
            // Agent loss overrides an unconsumed result: conservatively fail
            // closed for this lifecycle exception. Never touch actor/UI state.
            self.detached = true;
        }
    }
    fn poll(&self, now: Instant) -> FencePoll {
        let Some(expected) = &self.expected else {
            return FencePoll::Failed(Failure::HelperProtocol);
        };
        if self.detached {
            return FencePoll::Failed(Failure::HelperProtocol);
        }
        // Honor a result ACCEPTED before the deadline even if UI tick is late.
        match self.result {
            Some(true) => FencePoll::Acknowledged,
            Some(false) => FencePoll::Failed(Failure::HelperProtocol),
            None if now >= expected.deadline => FencePoll::Failed(Failure::HelperTimeout),
            None => FencePoll::Pending,
        }
    }
    fn clear(&mut self) {
        self.expected = None;
        self.result = None;
        self.detached = false;
    }
}
wrap_dev_tools_message_observer! {
    struct PasteFenceObserver { mailbox: Arc<Mutex<FenceMailbox>> }
    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(&self, browser: Option<&mut Browser>, id: i32,
            success: i32, _result: Option<&[u8]>) {
            let arrived = Instant::now();
            let Some(browser) = browser else { return; };
            // Only the read-only browser-ID getter is used for validation;
            // wrapper pointer addresses are NOT stable native browser identity.
            // No CEF commands, pumping, UI/model access or raw-result parsing.
            let key = FenceKey { browser_id: browser.identifier(), method_id: id };
            if let Ok(mut mailbox) = self.mailbox.lock() {
                mailbox.record_result(key, success == 1, arrived);
            }
        }
        fn on_dev_tools_agent_detached(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return; };
            let browser_id = browser.identifier();
            if let Ok(mut mailbox) = self.mailbox.lock() {
                mailbox.record_detached(browser_id);
            }
        }
    }
}
struct NativeInputFence {
    registration: Option<Registration>,
    mailbox: Arc<Mutex<FenceMailbox>>,
    host: BrowserHost,
    params: DictionaryValue,
    method: CefString,
    method_id: i32,
    deadline: Instant,
}
impl NativeInputFence {
    fn prepare(browser: &Browser, expected_browser_id: i32, now: Instant) -> Result<Self, Failure> {
        if browser.identifier() != expected_browser_id {
            return Err(Failure::HelperProtocol);
        }
        let method_id = checked_fence_method_id(next_id()?).ok_or(Failure::IdentifierExhausted)?;
        let deadline = now + SOURCE_FENCE_LIMIT;
        let key = FenceKey {
            browser_id: expected_browser_id,
            method_id,
        };
        let mailbox = Arc::new(Mutex::new(FenceMailbox::new(key, now, deadline)?));
        // Constant command-free input; no clipboard bytes, editing command,
        // modifiers, permission settings or website-origin data in these params.
        let params = parse_json(
            Some(&PASTE_FENCE_PARAMS.into()),
            JsonParserOptions::default(),
        )
        .and_then(|value| value.dictionary())
        .ok_or(Failure::HelperProtocol)?;
        let method = "Input.dispatchKeyEvent".into();
        let host = browser.host().ok_or(Failure::HelperProtocol)?;
        let mut observer = PasteFenceObserver::new(mailbox.clone());
        let registration = match host.add_dev_tools_message_observer(Some(&mut observer)) {
            Some(registration) => registration,
            None => {
                if let Ok(mut mailbox) = mailbox.lock() {
                    mailbox.clear();
                }
                return Err(Failure::HelperProtocol);
            }
        };
        let fence = Self {
            registration: Some(registration),
            mailbox,
            host,
            params,
            method,
            method_id,
            deadline,
        };
        // Unexpected result/detach before submission is not an input fence.
        if fence.poll(now) != FencePoll::Pending {
            return Err(Failure::HelperProtocol);
        }
        Ok(fence)
    }
    fn poll(&self, now: Instant) -> FencePoll {
        self.mailbox
            .lock()
            .map(|mailbox| mailbox.poll(now))
            .unwrap_or(FencePoll::Failed(Failure::HelperProtocol))
    }
}
impl Drop for NativeInputFence {
    fn drop(&mut self) {
        if let Ok(mut mailbox) = self.mailbox.lock() {
            mailbox.clear();
        }
        // Clearing first makes any detach/result callback during unregister inert.
        // No mailbox lock is held over registration destruction/native calls.
        drop(self.registration.take());
    }
}

struct Source {
    owner: OwnerStamp,
    browser_id: i32,
    browser: Browser,
    frame: Frame,
    frame_id: String,
}
impl Source {
    fn capture(owner: OwnerStamp, browser: &Browser) -> Result<Self, Failure> {
        let frame = browser.focused_frame().ok_or(Failure::OwnerChanged)?;
        if frame.is_valid() != 1 || frame.is_focused() != 1 {
            return Err(Failure::OwnerChanged);
        }
        let mut frame_browser = frame.browser().ok_or(Failure::OwnerChanged)?;
        if browser.is_same(Some(&mut frame_browser)) != 1 {
            return Err(Failure::OwnerChanged);
        }
        Ok(Self {
            owner,
            browser_id: browser.identifier(),
            browser: browser.clone(),
            frame_id: frame_id(&frame),
            frame,
        })
    }
    fn revalidate(&self, owner: OwnerStamp, browser: &Browser) -> Result<Frame, Failure> {
        if owner != self.owner || self.frame.is_valid() != 1 {
            return Err(Failure::OwnerChanged);
        }
        let mut captured = self.browser.clone();
        if browser.is_same(Some(&mut captured)) != 1 {
            return Err(Failure::OwnerChanged);
        }
        // Capture the CURRENT focused frame immediately before native dispatch.
        let current = Self::capture(owner, browser)?;
        if current.frame_id != self.frame_id {
            return Err(Failure::OwnerChanged);
        }
        Ok(current.frame)
    }
}
fn frame_id(frame: &Frame) -> String {
    CefString::from(&frame.identifier()).to_string()
}
struct Transaction {
    id: TransactionId,
    operation: Operation,
    source: Source,
    seed: String,
    seed_deadline: Instant,
    window_end: Option<Instant>,
    source_issued: bool,
    abandoned: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    ContextPending,
    Creating,
    Loading,
    Init,
    Idle,
    SeedArm,
    SeedCopy,
    SeedPasteArm,
    SeedPaste,
    Ready,
    ResultPasteArm,
    ResultPaste,
    SourcePasteFence,
    Poisoned,
    Closing,
    Closed,
}

/// One process-global instance; no queue and no implicit source dispatch. Busy
/// rejects new work. Parent may queue user actions externally, with revalidation.
/// !Send/!Sync: all CEF calls and tick/dispatch stay on the runtime UI thread.
pub struct Coordinator {
    context: RequestContext,
    browser: Option<Browser>,
    shared: Arc<Shared>,
    marker: String,
    phase: Phase,
    setup_deadline: Instant,
    native_event_deadline: Option<Instant>,
    source_fence: Option<NativeInputFence>,
    transaction: Option<Transaction>,
    completion: Option<Completion>,
    poison: Option<Failure>,
    _ui_thread: PhantomData<Rc<()>>,
}
impl Coordinator {
    /// CEF must already be initialized with its normal sandbox/environment
    /// checks. This never initializes CEF, changes switches, or pumps its loop.
    /// Construction is claimed once per process, even after failure/drop.
    pub fn new(now: Instant) -> Result<Self, Failure> {
        if CLAIMED.swap(true, Ordering::AcqRel) {
            return Err(Failure::AlreadyConstructed);
        }
        let marker = format!("{}-{}", std::process::id(), next_id()?);
        let shared = Arc::new(Shared::default());
        let mut handler = ContextReady::new(shared.clone());
        let context = request_context_create_context(
            Some(&RequestContextSettings::default()),
            Some(&mut handler),
        )
        .ok_or(Failure::HelperUnavailable)?;
        if context.is_global() != 0 {
            return Err(Failure::HelperUnavailable);
        }
        Ok(Self {
            context,
            browser: None,
            shared,
            marker,
            phase: Phase::ContextPending,
            setup_deadline: now + SETUP_LIMIT,
            native_event_deadline: None,
            source_fence: None,
            transaction: None,
            completion: None,
            poison: None,
            _ui_thread: PhantomData,
        })
    }
    pub fn status(&self) -> Status {
        if self.phase == Phase::Closing {
            return Status::Closing;
        }
        if self.phase == Phase::Closed {
            return Status::Closed;
        }
        if let Some(c) = &self.completion {
            let kind = match &c.result {
                CompletionResult::CopiedText(_) => CompletionKind::Text,
                CompletionResult::NoUpdateObserved => CompletionKind::NoUpdateObserved,
                CompletionResult::PasteDispatched => CompletionKind::PasteDispatched,
                CompletionResult::Failed(_) => CompletionKind::Failed,
            };
            return Status::Complete {
                transaction: c.transaction,
                kind,
                poisoned: c.coordinator_poisoned,
            };
        }
        if let Some(error) = self.poison {
            return Status::Poisoned(error);
        }
        if let Some(t) = self.transaction.as_ref().filter(|t| t.abandoned) {
            return Status::AbandonedDraining(t.id);
        }
        match (self.phase, self.transaction.as_ref()) {
            (Phase::Idle, _) => Status::Idle,
            (Phase::Ready, Some(t)) => Status::ReadyForSource(ReadyForSource {
                transaction: t.id,
                owner: t.source.owner,
                operation: t.operation,
                browser_id: t.source.browser_id,
                frame_id: t.source.frame_id.clone(),
            }),
            (Phase::ResultPasteArm | Phase::ResultPaste | Phase::SourcePasteFence, Some(t)) => {
                Status::WaitingResult(t.id)
            }
            (_, Some(t)) => Status::SeedPending(t.id),
            (Phase::ContextPending | Phase::Creating | Phase::Loading | Phase::Init, None) => {
                Status::SetupPending
            }
            _ => Status::Unavailable(Failure::HelperUnavailable),
        }
    }
    /// Serialize all app commands until completion is drained/canceled. This is
    /// not a lock on independent ordinary website/OS clipboard writers.
    pub fn is_busy(&self) -> bool {
        self.transaction.is_some() || self.completion.is_some()
    }
    pub fn begin_copy_cut(
        &mut self,
        owner: OwnerStamp,
        browser: &Browser,
        operation: Operation,
        now: Instant,
    ) -> Result<TransactionId, Failure> {
        if operation == Operation::Paste {
            return Err(Failure::HelperProtocol);
        }
        self.begin(owner, browser, operation, None, now)
    }
    pub fn begin_paste(
        &mut self,
        owner: OwnerStamp,
        browser: &Browser,
        plaintext: Option<String>,
        now: Instant,
    ) -> Result<TransactionId, Failure> {
        let text = plaintext.ok_or(Failure::NoPlaintext)?;
        self.begin(owner, browser, Operation::Paste, Some(text), now)
    }
    fn begin(
        &mut self,
        owner: OwnerStamp,
        browser: &Browser,
        operation: Operation,
        paste: Option<String>,
        now: Instant,
    ) -> Result<TransactionId, Failure> {
        if self.poison.is_some() {
            return Err(Failure::RestartRequired);
        }
        if self.phase == Phase::Closing || self.phase == Phase::Closed {
            return Err(Failure::ShuttingDown);
        }
        if self.is_busy() {
            return Err(Failure::Busy);
        }
        if self.phase != Phase::Idle {
            return Err(Failure::NotReady);
        }
        if self.shared.fault.load(Ordering::Acquire) || self.shared.closed.load(Ordering::Acquire) {
            self.fail(Failure::HelperUnavailable, true);
            return Err(Failure::HelperUnavailable);
        }
        if paste.as_ref().is_some_and(|s| !bounded(s)) {
            return Err(Failure::PayloadTooLarge);
        }
        self.check_separate_context(browser)?;
        let source = Source::capture(owner, browser)?;
        let id = TransactionId(next_id()?);
        let seed = paste.unwrap_or_else(|| sentinel(id));
        self.transaction = Some(Transaction {
            id,
            operation,
            source,
            seed: seed.clone(),
            seed_deadline: now + SEED_LIMIT,
            window_end: None,
            source_issued: false,
            abandoned: false,
        });
        self.phase = Phase::SeedArm;
        if let Err(error) = self.send(WireOp::ArmCopy, &seed, now) {
            self.fail(error, true);
        }
        Ok(id)
    }
    fn check_separate_context(&self, browser: &Browser) -> Result<(), Failure> {
        let mut source_context = browser
            .host()
            .and_then(|h| h.request_context())
            .ok_or(Failure::OwnerChanged)?;
        if self.context.is_sharing_with(Some(&mut source_context)) != 0 {
            return Err(Failure::OwnerChanged);
        }
        if self
            .browser
            .as_ref()
            .is_some_and(|helper| helper.identifier() == browser.identifier())
        {
            return Err(Failure::OwnerChanged);
        }
        Ok(())
    }
    /// REQUIRED parent authorization boundary. Parent first rechecks CURRENT
    /// native page/app/document/focus ownership, then passes that current stamp
    /// and actual current native Browser. The actor independently checks its
    /// capture and current focused Frame. It never acts on Ready by itself.
    pub fn dispatch_source(
        &mut self,
        id: TransactionId,
        current_owner: OwnerStamp,
        current_browser: &Browser,
        now: Instant,
    ) -> Result<(), Failure> {
        if self.phase != Phase::Ready {
            return Err(Failure::NotReady);
        }
        let t = self.transaction.as_ref().ok_or(Failure::NotReady)?;
        if t.id != id || t.abandoned {
            return Err(Failure::OwnerChanged);
        }
        if self.shared.fault.load(Ordering::Acquire) || self.shared.closed.load(Ordering::Acquire) {
            self.fail(Failure::HelperUnavailable, true);
            return Err(Failure::HelperUnavailable);
        }
        if now >= t.seed_deadline {
            self.fail(Failure::Deadline, false);
            return Err(Failure::Deadline);
        }
        if let Err(error) = self.check_separate_context(current_browser) {
            self.fail(error, false);
            return Err(error);
        }
        let operation = t.operation;
        if operation == Operation::Paste {
            // Build/attach EVERYTHING before the final focused-frame check.
            // No clipboard payload is carried in this debugger input command.
            let prepared = NativeInputFence::prepare(&t.source.browser, t.source.browser_id, now);
            match prepared {
                Ok(fence) => self.source_fence = Some(fence),
                Err(error) => {
                    self.fail(error, true);
                    return Err(error);
                }
            }
        }
        let frame = match self
            .transaction
            .as_ref()
            .ok_or(Failure::NotReady)
            .and_then(|t| t.source.revalidate(current_owner, current_browser))
        {
            Ok(frame) => frame,
            Err(error) => {
                self.fail(error, false);
                return Err(error);
            }
        };
        if operation == Operation::Paste {
            // Rust/mailbox-only check after final native-frame validation. A
            // detach or expired preparation cannot authorize the source action.
            let prepared = self
                .source_fence
                .as_ref()
                .map(|fence| fence.poll(Instant::now()));
            let error = match prepared {
                Some(FencePoll::Pending) => None,
                Some(FencePoll::Failed(error)) => Some(error),
                _ => Some(Failure::HelperProtocol),
            };
            if let Some(error) = error {
                self.fail(error, true);
                return Err(error);
            }
        }
        if let Some(t) = &mut self.transaction {
            t.source_issued = true;
            t.window_end = Some(now + OBSERVATION_WINDOW);
        }
        // BROWSER native path, never renderer-side Frame.Paste. Observer
        // callbacks only validate IDs and publish to an independent mailbox;
        // no command dispatch, pumping or actor/UI access. No mutex is held
        // over either call.
        if operation == Operation::Paste {
            self.phase = Phase::SourcePasteFence;
            let Some(fence) = self.source_fence.as_mut() else {
                self.fail(Failure::HelperProtocol, true);
                return Err(Failure::HelperProtocol);
            };
            self.native_event_deadline = Some(fence.deadline);
            // REQUIRED ADJACENT UI-THREAD PAIR. Do not insert getters, helper
            // calls, a loop/pump, source JS, parsing or registration between them.
            frame.paste();
            let returned = fence.host.execute_dev_tools_method(
                fence.method_id,
                Some(&fence.method),
                Some(&mut fence.params),
            );
            if returned != fence.method_id {
                self.fail(Failure::HelperProtocol, true);
                return Err(Failure::HelperProtocol);
            }
            // Submission is not completion. No helper seed/reuse until tick
            // consumes the correlated native result or conservatively poisons.
        } else {
            match operation {
                Operation::Copy => frame.copy(),
                Operation::Cut => frame.cut(),
                Operation::Paste => {
                    self.fail(Failure::HelperProtocol, true);
                    return Err(Failure::HelperProtocol);
                }
            }
            self.phase = Phase::ResultPasteArm;
            if let Err(error) = self.send(WireOp::ArmPaste, "", now) {
                self.fail(error, true);
            }
        }
        Ok(())
    }
    /// Ownership invalidation forbids export but does NOT poison. Outstanding
    /// helper/source work drains on global tick even if the pane drops its ID.
    /// New work stays Busy until a healthy result/no-update observation settles.
    /// Only a genuine helper/seed/transport failure during draining poisons.
    pub fn cancel(&mut self, id: TransactionId) {
        if self
            .completion
            .as_ref()
            .is_some_and(|c| c.transaction == id)
        {
            self.completion = None;
            return;
        }
        let Some(t) = self.transaction.as_mut().filter(|t| t.id == id) else {
            return;
        };
        let pending = self
            .shared
            .mailbox
            .lock()
            .map(|m| m.pending.is_some())
            .unwrap_or(true);
        let drain = must_drain_on_abandonment(
            t.source_issued,
            pending,
            self.native_event_deadline.is_some(),
        );
        t.abandoned = true;
        if !drain {
            self.finish(CompletionResult::Failed(Failure::Cancelled), false);
        }
        // Otherwise leave phase, deadlines and native IPC intact. No source
        // command is issued by tick; an abandoned seed can never become Ready.
    }
    /// Parent must revalidate the returned owner BEFORE exporting text to GPUI.
    /// cancel(id) before this call removes the completion and prevents export.
    /// No other transaction starts while this completion is pending consumption.
    pub fn take_completion(&mut self, id: TransactionId) -> Option<Completion> {
        if self.completion.as_ref()?.transaction != id {
            return None;
        }
        self.completion.take()
    }
    /// One nonblocking advancement after the OUTER runtime pumps CEF. Never call
    /// cef::do_message_loop_work here or from a callback. No sleeps/retries of Cut.
    pub fn tick(&mut self, now: Instant) {
        if self.phase == Phase::Closing {
            if self.is_drained() {
                self.browser = None;
                self.phase = Phase::Closed;
            }
            return;
        }
        if self.phase == Phase::Closed || self.poison.is_some() {
            return;
        }
        if self.shared.closed.load(Ordering::Acquire) || self.shared.fault.load(Ordering::Acquire) {
            self.fail(Failure::HelperUnavailable, true);
            return;
        }
        if self.phase == Phase::SourcePasteFence {
            self.poll_source_fence(now);
            return;
        }
        if matches!(
            self.phase,
            Phase::ContextPending | Phase::Creating | Phase::Loading | Phase::Init
        ) && now >= self.setup_deadline
        {
            self.fail(Failure::HelperTimeout, true);
            return;
        }
        if self
            .transaction
            .as_ref()
            .is_some_and(|t| !t.source_issued && now >= t.seed_deadline)
        {
            self.fail(
                if self.phase == Phase::Ready {
                    Failure::Deadline
                } else {
                    Failure::SeedNotVerified
                },
                self.phase != Phase::Ready,
            );
            return;
        }
        if self.phase == Phase::ContextPending && self.shared.context_ready.load(Ordering::Acquire)
        {
            if let Err(error) = self.create_browser() {
                self.fail(error, true);
            }
            return;
        }
        if self.phase == Phase::Creating {
            let created = self.shared.created_browser.lock().ok().and_then(|id| *id);
            if let Some(id) = created {
                match browser_host_get_browser_by_identifier(id) {
                    Some(browser) => {
                        self.browser = Some(browser);
                        self.phase = Phase::Loading;
                    }
                    None => self.fail(Failure::HelperUnavailable, true),
                }
            }
            return;
        }
        if self.phase == Phase::Loading && self.shared.loaded.load(Ordering::Acquire) {
            self.phase = Phase::Init;
            if let Err(error) = self.send(WireOp::Init, "", now) {
                self.fail(error, true);
            }
            return;
        }
        let received = match self.shared.mailbox.lock() {
            Err(_) => Err(Failure::HelperProtocol),
            Ok(mut mailbox) => {
                // A reply accepted before its deadline remains valid if the UI
                // processes the mailbox later. Do not mislabel that IPC timeout.
                if let Some(reply) = mailbox.reply.take() {
                    mailbox.pending = None;
                    Ok(Some(reply))
                } else if mailbox.pending.as_ref().is_some_and(|p| now >= p.deadline) {
                    Err(Failure::HelperTimeout)
                } else {
                    Ok(None)
                }
            }
        };
        match received {
            Err(error) => self.fail(error, true),
            Ok(Some(reply)) => {
                if let Err(error) = self.advance(reply, now) {
                    self.fail(error, true);
                }
            }
            Ok(None) => {
                if self.native_event_deadline.is_some_and(|end| now >= end) {
                    self.fail(Failure::HelperTimeout, true);
                }
            }
        }
    }
    fn poll_source_fence(&mut self, now: Instant) {
        // Closing/disposed source frames are not a normal-delivery guarantee.
        // Owner invalidation alone instead marks abandoned and keeps draining.
        if !self
            .transaction
            .as_ref()
            .is_some_and(|t| t.source.frame.is_valid() == 1)
        {
            self.fail(Failure::HelperProtocol, true);
            return;
        }
        let result = self
            .source_fence
            .as_ref()
            .map(|fence| fence.poll(now))
            .unwrap_or(FencePoll::Failed(Failure::HelperProtocol));
        match result {
            FencePoll::Pending => {}
            FencePoll::Acknowledged => self.finish(CompletionResult::PasteDispatched, false),
            FencePoll::Failed(error) => self.fail(error, true),
        }
    }
    fn create_browser(&mut self) -> Result<(), Failure> {
        self.context.set_content_setting(
            None,
            None,
            ContentSettingTypes::CLIPBOARD_READ_WRITE,
            ContentSettingValues::BLOCK,
        );
        let mut extra = dictionary_value_create().ok_or(Failure::HelperUnavailable)?;
        extra.set_int(Some(&VERSION_KEY.into()), PROTOCOL_VERSION);
        extra.set_string(Some(&MARKER_KEY.into()), Some(&self.marker.as_str().into()));
        let mut client = HelperClient::new(self.shared.clone());
        self.shared.creating.store(true, Ordering::Release);
        CREATING_HELPERS.fetch_add(1, Ordering::AcqRel);
        let accepted = browser_host_create_browser(
            Some(&WindowInfo::default().set_as_windowless(0)),
            Some(&mut client),
            Some(&"about:blank".into()),
            Some(&BrowserSettings {
                windowless_frame_rate: 1,
                javascript_access_clipboard: State::DISABLED,
                javascript_dom_paste: State::DISABLED,
                ..Default::default()
            }),
            Some(&mut extra),
            Some(&mut self.context),
        );
        if accepted != 1 {
            if self.shared.creating.swap(false, Ordering::AcqRel) {
                CREATING_HELPERS.fetch_sub(1, Ordering::AcqRel);
            }
            return Err(Failure::HelperUnavailable);
        }
        self.phase = Phase::Creating;
        Ok(())
    }
    fn helper_frame(&self) -> Result<Frame, Failure> {
        let frame = self
            .browser
            .as_ref()
            .and_then(|b| b.main_frame())
            .ok_or(Failure::HelperUnavailable)?;
        if frame.is_valid() != 1 || frame.is_main() != 1 {
            return Err(Failure::HelperUnavailable);
        }
        Ok(frame)
    }
    fn send(&mut self, op: WireOp, text: &str, now: Instant) -> Result<(), Failure> {
        if !bounded(text) {
            return Err(Failure::PayloadTooLarge);
        }
        let frame = self.helper_frame()?;
        let key = WireKey {
            browser: self
                .browser
                .as_ref()
                .ok_or(Failure::HelperUnavailable)?
                .identifier(),
            frame: frame_id(&frame),
            marker: self.marker.clone(),
            transaction: self.transaction.as_ref().map_or(0, |t| t.id.0),
            nonce: next_id()?,
            op,
        };
        let mut message =
            process_message_create(Some(&CALL.into())).ok_or(Failure::HelperProtocol)?;
        let args = message.argument_list().ok_or(Failure::HelperProtocol)?;
        args.set_int(0, PROTOCOL_VERSION);
        args.set_int(1, key.browser);
        for (index, value) in [
            (2, key.frame.as_str()),
            (3, key.marker.as_str()),
            (4, key.transaction.to_string().as_str()),
            (5, key.nonce.to_string().as_str()),
        ] {
            args.set_string(index, Some(&value.into()));
        }
        args.set_int(6, op as i32);
        args.set_string(7, Some(&text.into()));
        {
            let mut mailbox = self
                .shared
                .mailbox
                .lock()
                .map_err(|_| Failure::HelperProtocol)?;
            if mailbox.pending.is_some() {
                return Err(Failure::HelperProtocol);
            }
            mailbox.pending = Some(Pending {
                key,
                deadline: now + IPC_LIMIT,
            });
            mailbox.reply = None;
        }
        frame.send_process_message(ProcessId::RENDERER, Some(&mut message));
        Ok(())
    }
    fn native_helper_copy(&mut self, now: Instant) -> Result<(), Failure> {
        self.native_event_deadline = Some(now + EVENT_LIMIT);
        self.helper_frame()?.copy();
        self.send(WireOp::Snapshot, "", now)
    }
    fn native_helper_paste(&mut self, now: Instant) -> Result<(), Failure> {
        self.native_event_deadline = Some(now + EVENT_LIMIT);
        self.helper_frame()?.paste();
        self.send(WireOp::Snapshot, "", now)
    }
    fn advance(&mut self, reply: WireReply, now: Instant) -> Result<(), Failure> {
        if !reply.ok || !valid_reply_shape(&reply) {
            return Err(Failure::HelperProtocol);
        }
        let event_unsettled = (self.phase == Phase::SeedCopy && !reply.copy_seen)
            || (matches!(self.phase, Phase::SeedPaste | Phase::ResultPaste) && !reply.paste_seen);
        if event_unsettled && self.native_event_deadline.is_some_and(|end| now >= end) {
            return Err(Failure::HelperTimeout);
        }
        match self.phase {
            Phase::Init => self.phase = Phase::Idle,
            Phase::SeedArm => {
                self.phase = Phase::SeedCopy;
                self.native_helper_copy(now)?;
            }
            Phase::SeedCopy => {
                if !reply.copy_seen {
                    self.send(WireOp::Snapshot, "", now)?;
                } else {
                    self.native_event_deadline = None;
                    self.phase = Phase::SeedPasteArm;
                    self.send(WireOp::ArmPaste, "", now)?;
                }
            }
            Phase::SeedPasteArm => {
                self.phase = Phase::SeedPaste;
                self.native_helper_paste(now)?;
            }
            Phase::SeedPaste => {
                if !reply.paste_seen {
                    self.send(WireOp::Snapshot, "", now)?;
                } else {
                    self.native_event_deadline = None;
                    let t = self.transaction.as_ref().ok_or(Failure::HelperProtocol)?;
                    if seed_verified(&reply, &t.seed) {
                        if t.abandoned {
                            self.finish(CompletionResult::Failed(Failure::Cancelled), false);
                        } else {
                            self.phase = Phase::Ready;
                        }
                    } else {
                        self.phase = Phase::SeedPasteArm;
                        self.send(WireOp::ArmPaste, "", now)?;
                    }
                }
            }
            Phase::ResultPasteArm => {
                self.phase = Phase::ResultPaste;
                self.native_helper_paste(now)?;
            }
            Phase::ResultPaste => {
                if !reply.paste_seen {
                    self.send(WireOp::Snapshot, "", now)?;
                } else {
                    self.native_event_deadline = None;
                    let t = self.transaction.as_ref().ok_or(Failure::HelperProtocol)?;
                    let ended = now >= t.window_end.ok_or(Failure::HelperProtocol)?;
                    match observe_result(&reply, &t.seed, ended) {
                        Observation::Text => {
                            self.finish(CompletionResult::CopiedText(reply.text), false)
                        }
                        Observation::NoUpdate => {
                            self.finish(CompletionResult::NoUpdateObserved, false)
                        }
                        Observation::NoPlaintext => self.fail(Failure::NoPlaintext, false),
                        Observation::Oversized => self.fail(Failure::PayloadTooLarge, false),
                        Observation::Continue => {
                            self.phase = Phase::ResultPasteArm;
                            self.send(WireOp::ArmPaste, "", now)?;
                        }
                    }
                }
            }
            _ => return Err(Failure::HelperProtocol),
        }
        Ok(())
    }
    fn clear_mailbox(&self) {
        if let Ok(mut mailbox) = self.shared.mailbox.lock() {
            mailbox.pending = None;
            mailbox.reply = None;
        }
    }
    fn finish(&mut self, result: CompletionResult, poison: bool) {
        // Unregister BEFORE the source Browser/Frame lease is released below.
        drop(self.source_fence.take());
        self.clear_mailbox();
        self.native_event_deadline = None;
        if let Some(t) = self.transaction.take() {
            self.completion = make_completion(
                t.id,
                t.source.owner,
                t.operation,
                NativeFrameIdentity {
                    browser_id: t.source.browser_id,
                    frame_id: t.source.frame_id.clone(),
                },
                result,
                t.abandoned,
                poison,
            );
            // Abandoned payload and source Browser/Frame lease drop here.
        }
        self.phase = if poison { Phase::Poisoned } else { Phase::Idle };
    }
    fn fail(&mut self, error: Failure, poison: bool) {
        if poison {
            self.poison = Some(error);
        }
        self.finish(CompletionResult::Failed(error), poison);
        if poison {
            if let Some(completion) = &mut self.completion {
                completion.coordinator_poisoned = true;
            }
            self.close_helper();
        }
    }
    /// Nonblocking close request. No clipboard scrub and no process termination.
    /// No reset/recreate API: poisoned instances require app restart for now.
    pub fn shutdown(&mut self) {
        // Disable stale callbacks/unregister before dropping source CEF refs.
        drop(self.source_fence.take());
        self.transaction = None;
        self.completion = None;
        self.clear_mailbox();
        self.native_event_deadline = None;
        if matches!(self.phase, Phase::Closing | Phase::Closed) {
            return;
        }
        self.phase = Phase::Closing;
        self.close_helper();
        if self.is_drained() {
            self.phase = Phase::Closed;
        }
    }
    fn close_helper(&self) {
        // Also covers a late async browser-created callback after actor timeout
        // or drop. That callback observes close_requested and closes itself.
        self.shared.close_requested.store(true, Ordering::Release);
        let browser = self.browser.clone().or_else(|| {
            let id = self.shared.created_browser.lock().ok().and_then(|id| *id)?;
            browser_host_get_browser_by_identifier(id)
        });
        if let Some(host) = browser.and_then(|b| b.host()) {
            host.close_browser(1);
        }
    }
    pub fn is_drained(&self) -> bool {
        shared_drained(&self.shared)
    }
    pub fn closing_count(&self) -> usize {
        usize::from(!self.is_drained())
    }
}
impl Drop for Coordinator {
    fn drop(&mut self) {
        self.shutdown();
    }
}
/// Include this count in the OUTER runtime's shutdown drain condition, including
/// after accidental coordinator drop. Also drain pending_helper_creation_count!
pub fn live_helper_count() -> usize {
    LIVE_HELPERS.load(Ordering::Acquire)
}
pub fn pending_helper_creation_count() -> usize {
    CREATING_HELPERS.load(Ordering::Acquire)
}
/// Never call cef::shutdown until this reaches zero, including pending creates.
pub fn outstanding_helper_count() -> usize {
    live_helper_count() + pending_helper_creation_count()
}
fn shared_drained(shared: &Shared) -> bool {
    !shared.creating.load(Ordering::Acquire) && !shared.live.load(Ordering::Acquire)
}
fn sentinel(id: TransactionId) -> String {
    format!("herdr-shuttle-{}-{}", std::process::id(), id.0)
}
fn must_drain_on_abandonment(issued: bool, pending: bool, native_pending: bool) -> bool {
    issued || pending || native_pending
}
fn make_completion(
    id: TransactionId,
    owner: OwnerStamp,
    operation: Operation,
    source: NativeFrameIdentity,
    result: CompletionResult,
    abandoned: bool,
    poisoned: bool,
) -> Option<Completion> {
    if abandoned {
        return None;
    }
    Some(Completion {
        transaction: id,
        owner,
        operation,
        source,
        result,
        coordinator_poisoned: poisoned,
    })
}
#[derive(Debug, PartialEq, Eq)]
enum Observation {
    Continue,
    Text,
    NoUpdate,
    NoPlaintext,
    Oversized,
}
fn valid_reply_shape(reply: &WireReply) -> bool {
    bounded(&reply.text)
        && (!reply.has_plaintext || reply.has_formats)
        && (!reply.has_formats || reply.paste_seen)
        && (!reply.oversized || reply.has_plaintext)
        && ((reply.has_plaintext && !reply.oversized) || reply.text.is_empty())
}
fn seed_verified(reply: &WireReply, seed: &str) -> bool {
    reply.ok
        && reply.copy_seen
        && reply.paste_seen
        && valid_reply_shape(reply)
        && bounded(seed)
        && !reply.oversized
        && ((reply.has_plaintext && reply.text == seed) || (seed.is_empty() && !reply.has_formats))
}
fn observe_result(reply: &WireReply, seed: &str, ended: bool) -> Observation {
    if !reply.ok || !valid_reply_shape(reply) {
        return Observation::NoPlaintext;
    }
    if !reply.paste_seen {
        return Observation::Continue;
    }
    if reply.oversized {
        return Observation::Oversized;
    }
    // Copy/Cut starts with a verified nonempty plain sentinel. NO native
    // formats is thus an observed clear, not sentinel/no-update or rich data.
    // Wait out the HEALTHY observation window: an early no-format read may be
    // transient while a normal copy commits. Shape validation guarantees text
    // is empty; at window end the existing Text path projects CopiedText("").
    if !reply.has_formats {
        return if ended {
            Observation::Text
        } else {
            Observation::Continue
        };
    }
    if reply.has_plaintext && reply.text != seed {
        return Observation::Text;
    }
    if !ended {
        return Observation::Continue;
    }
    if reply.has_plaintext {
        Observation::NoUpdate
    } else {
        Observation::NoPlaintext
    }
}

// These callbacks never acquire actor/runtime UI state or retain text in logs.
wrap_request_context_handler! { struct ContextReady { shared: Arc<Shared> } impl RequestContextHandler {
    fn on_request_context_initialized(&self, _context: Option<&mut RequestContext>) {
        self.shared.context_ready.store(true, Ordering::Release);
    }
}}
wrap_render_handler! { struct HelperPixels {} impl RenderHandler {
    fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
        if let Some(rect) = rect { *rect = Rect { x: 0, y: 0, width: 16, height: 16 }; }
    }
    fn on_paint(&self, _browser: Option<&mut Browser>, _kind: PaintElementType,
        _dirty: Option<&[Rect]>, _buffer: *const u8, _width: i32, _height: i32) {}
}}
wrap_load_handler! { struct HelperLoad { shared: Arc<Shared> } impl LoadHandler {
    fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _code: i32) {
        if let Some(frame) = frame {
            if frame.is_main() == 1 && CefString::from(&frame.url()).to_string() == "about:blank" {
                self.shared.loaded.store(true, Ordering::Release);
            } else { self.shared.fault.store(true, Ordering::Release); }
        }
    }
    fn on_load_error(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
        _code: Errorcode, _text: Option<&CefString>, _url: Option<&CefString>) {
        self.shared.fault.store(true, Ordering::Release);
    }
}}
wrap_life_span_handler! { struct HelperLife { shared: Arc<Shared> } impl LifeSpanHandler {
    fn on_after_created(&self, browser: Option<&mut Browser>) {
        if let Some(browser) = browser {
            // Keep total outstanding >0 throughout creating -> live handoff.
            if !self.shared.live.swap(true, Ordering::AcqRel) { LIVE_HELPERS.fetch_add(1, Ordering::AcqRel); }
            if self.shared.creating.swap(false, Ordering::AcqRel) { CREATING_HELPERS.fetch_sub(1, Ordering::AcqRel); }
            if let Ok(mut id) = self.shared.created_browser.lock() { *id = Some(browser.identifier()); }
            if let Some(host) = browser.host() {
                host.was_hidden(1);
                if self.shared.close_requested.load(Ordering::Acquire) { host.close_browser(1); }
            }
        } else {
            if self.shared.creating.swap(false, Ordering::AcqRel) { CREATING_HELPERS.fetch_sub(1, Ordering::AcqRel); }
            self.shared.fault.store(true, Ordering::Release);
        }
    }
    fn on_before_close(&self, _browser: Option<&mut Browser>) {
        self.shared.closed.store(true, Ordering::Release);
        if self.shared.live.swap(false, Ordering::AcqRel) { LIVE_HELPERS.fetch_sub(1, Ordering::AcqRel); }
    }
    fn on_before_popup(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _id: i32,
        _url: Option<&CefString>, _name: Option<&CefString>, _disposition: WindowOpenDisposition,
        _gesture: i32, _features: Option<&PopupFeatures>, _info: Option<&mut WindowInfo>,
        _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>,
        _extra: Option<&mut Option<DictionaryValue>>, _access: Option<&mut i32>) -> i32 { 1 }
}}
wrap_permission_handler! { struct HelperPermissions {} impl PermissionHandler {
    fn on_show_permission_prompt(&self, _browser: Option<&mut Browser>, _id: u64,
        _origin: Option<&CefString>, _requested: u32, callback: Option<&mut PermissionPromptCallback>) -> i32 {
        if let Some(callback) = callback { callback.cont(PermissionRequestResult::DENY); } 1
    }
    fn on_request_media_access_permission(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
        _origin: Option<&CefString>, _requested: u32, callback: Option<&mut MediaAccessCallback>) -> i32 {
        if let Some(callback) = callback { callback.cont(0); } 1
    }
}}
wrap_resource_request_handler! { struct HelperResources {} impl ResourceRequestHandler {
    fn on_before_resource_load(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
        _request: Option<&mut Request>, _callback: Option<&mut Callback>) -> ReturnValue { ReturnValue::CANCEL }
    fn on_protocol_execution(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
        _request: Option<&mut Request>, allow: Option<&mut i32>) { if let Some(allow) = allow { *allow = 0; } }
}}
wrap_request_handler! { struct HelperRequests { shared: Arc<Shared> } impl RequestHandler {
    fn on_before_browse(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>,
        request: Option<&mut Request>, _gesture: i32, _redirect: i32) -> i32 {
        let initial = frame.is_some_and(|f| f.is_main() == 1)
            && request.is_some_and(|r| CefString::from(&r.url()).to_string() == "about:blank")
            && !self.shared.initial_navigation.swap(true, Ordering::AcqRel);
        if !initial { self.shared.fault.store(true, Ordering::Release); }
        i32::from(!initial)
    }
    fn resource_request_handler(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>,
        _request: Option<&mut Request>, _nav: i32, _download: i32, _origin: Option<&CefString>,
        _disable: Option<&mut i32>) -> Option<ResourceRequestHandler> { Some(HelperResources::new()) }
}}
wrap_download_handler! { struct HelperDownloads {} impl DownloadHandler {
    fn can_download(&self, _browser: Option<&mut Browser>, _url: Option<&CefString>, _method: Option<&CefString>) -> i32 { 0 }
    fn on_before_download(&self, _browser: Option<&mut Browser>, _item: Option<&mut DownloadItem>,
        _name: Option<&CefString>, _callback: Option<&mut BeforeDownloadCallback>) -> i32 { 1 }
}}
wrap_dialog_handler! { struct HelperDialogs {} impl DialogHandler {
    fn on_file_dialog(&self, _browser: Option<&mut Browser>, _mode: FileDialogMode,
        _title: Option<&CefString>, _path: Option<&CefString>, _filters: Option<&mut CefStringList>,
        _extensions: Option<&mut CefStringList>, _descriptions: Option<&mut CefStringList>,
        callback: Option<&mut FileDialogCallback>) -> i32 {
        if let Some(callback) = callback { callback.cancel(); } 1
    }
}}
wrap_display_handler! { struct HelperDisplay {} impl DisplayHandler {
    fn on_console_message(&self, _browser: Option<&mut Browser>, _level: LogSeverity,
        _message: Option<&CefString>, _source: Option<&CefString>, _line: i32) -> i32 { 1 }
}}
wrap_client! { struct HelperClient { shared: Arc<Shared> } impl Client {
    fn render_handler(&self) -> Option<RenderHandler> { Some(HelperPixels::new()) }
    fn load_handler(&self) -> Option<LoadHandler> { Some(HelperLoad::new(self.shared.clone())) }
    fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(HelperLife::new(self.shared.clone())) }
    fn permission_handler(&self) -> Option<PermissionHandler> { Some(HelperPermissions::new()) }
    fn request_handler(&self) -> Option<RequestHandler> { Some(HelperRequests::new(self.shared.clone())) }
    fn download_handler(&self) -> Option<DownloadHandler> { Some(HelperDownloads::new()) }
    fn dialog_handler(&self) -> Option<DialogHandler> { Some(HelperDialogs::new()) }
    fn display_handler(&self) -> Option<DisplayHandler> { Some(HelperDisplay::new()) }
    fn on_process_message_received(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>,
        source: ProcessId, message: Option<&mut ProcessMessage>) -> i32 {
        if source != ProcessId::RENDERER { return 0; }
        let (Some(browser), Some(frame), Some(message)) = (browser, frame, message) else { return 0; };
        if CefString::from(&message.name()).to_string() != REPLY { return 0; }
        if frame.is_valid() != 1 || frame.is_main() != 1 { return 1; }
        let Some(args) = message.argument_list() else { return 1; };
        // [version, marker, tx, nonce, op, ok, copy_seen, paste_seen,
        //  has_plaintext, oversized, has_formats, text]; v2 exact types/size.
        if args.size() != 12 || args.get_type(0) != ValueType::INT
            || args.get_type(4) != ValueType::INT
            || [1,2,3,11].into_iter().any(|i| args.get_type(i) != ValueType::STRING)
            || (5..11).any(|i| args.get_type(i) != ValueType::BOOL)
            || args.int(0) != PROTOCOL_VERSION { return 1; }
        let Some(op) = WireOp::from_i32(args.int(4)) else { return 1; };
        let string = |i| CefString::from(&args.string(i)).to_string();
        let (Some(transaction), Some(nonce)) = (decimal(&string(2), true), decimal(&string(3), false)) else { return 1; };
        let text = string(11); let marker = string(1);
        let reply = WireReply { ok: args.bool(5) == 1,
            copy_seen: args.bool(6) == 1, paste_seen: args.bool(7) == 1,
            has_plaintext: args.bool(8) == 1, oversized: args.bool(9) == 1,
            has_formats: args.bool(10) == 1, text };
        if !valid_reply_shape(&reply) || marker.is_empty() || marker.len() > 128 { return 1; }
        let key = WireKey { browser: browser.identifier(), frame: frame_id(frame),
            marker, transaction, nonce, op };
        if !frame.browser().is_some_and(|b| b.identifier() == browser.identifier()) { return 1; }
        if let Ok(mut mailbox) = self.shared.mailbox.lock() {
            if mailbox.reply.is_none() && mailbox.pending.as_ref().is_some_and(|p| accepts(p, &key, Instant::now())) {
                mailbox.reply = Some(reply);
            }
        }
        1
    }
}}

// RENDERER SIDE. Parent forwards its RenderProcessHandler callbacks here.
// Registrations derive ONLY from native browser-created extra_info. Ordinary
// webpages cannot install registrations or invoke a privileged page callback.
struct RendererIdentity {
    marker: String,
    frame: Option<String>,
    last_nonce: u64,
}
#[derive(Default)]
pub struct RendererHooks {
    helpers: Mutex<HashMap<i32, RendererIdentity>>,
}
impl RendererHooks {
    pub fn on_browser_created(&self, browser: &Browser, extra: Option<&DictionaryValue>) {
        let Some(extra) = extra else {
            return;
        };
        if extra.size() != 2
            || extra.get_type(Some(&VERSION_KEY.into())) != ValueType::INT
            || extra.int(Some(&VERSION_KEY.into())) != PROTOCOL_VERSION
            || extra.get_type(Some(&MARKER_KEY.into())) != ValueType::STRING
        {
            return;
        }
        let marker = CefString::from(&extra.string(Some(&MARKER_KEY.into()))).to_string();
        if marker.is_empty() || marker.len() > 128 {
            return;
        }
        if let Ok(mut helpers) = self.helpers.lock() {
            helpers.insert(
                browser.identifier(),
                RendererIdentity {
                    marker,
                    frame: None,
                    last_nonce: 0,
                },
            );
        }
    }
    pub fn on_browser_destroyed(&self, browser: &Browser) {
        if let Ok(mut helpers) = self.helpers.lock() {
            helpers.remove(&browser.identifier());
        }
    }
    /// Return true only for this private protocol. No registry mutex is held
    /// while evaluating JS or sending a CEF message (callbacks may be reentrant).
    pub fn on_process_message_received(
        &self,
        browser: &Browser,
        frame: &Frame,
        source: ProcessId,
        message: &ProcessMessage,
    ) -> bool {
        if source != ProcessId::BROWSER || CefString::from(&message.name()).to_string() != CALL {
            return false;
        }
        if frame.is_valid() != 1
            || frame.is_main() != 1
            || !frame
                .browser()
                .is_some_and(|b| b.identifier() == browser.identifier())
        {
            return true;
        }
        let Some(args) = message.argument_list() else {
            return true;
        };
        if args.size() != 8
            || [0, 1, 6]
                .into_iter()
                .any(|i| args.get_type(i) != ValueType::INT)
            || [2, 3, 4, 5, 7]
                .into_iter()
                .any(|i| args.get_type(i) != ValueType::STRING)
            || args.int(0) != PROTOCOL_VERSION
            || args.int(1) != browser.identifier()
        {
            return true;
        }
        let string = |i| CefString::from(&args.string(i)).to_string();
        let marker = string(3);
        let text = string(7);
        if marker.len() > 128 || !bounded(&text) || string(2) != frame_id(frame) {
            return true;
        }
        let (Some(transaction), Some(nonce), Some(op)) = (
            decimal(&string(4), true),
            decimal(&string(5), false),
            WireOp::from_i32(args.int(6)),
        ) else {
            return true;
        };
        if (op != WireOp::Init && transaction == 0) || (op != WireOp::ArmCopy && !text.is_empty()) {
            return true;
        }
        {
            let Ok(mut helpers) = self.helpers.lock() else {
                return true;
            };
            let Some(identity) = helpers.get_mut(&browser.identifier()) else {
                return true;
            };
            if identity.marker != marker || nonce <= identity.last_nonce {
                return true;
            }
            if let Some(bound) = &identity.frame {
                if *bound != frame_id(frame) || op == WireOp::Init {
                    return true;
                }
            } else {
                if op != WireOp::Init || transaction != 0 {
                    return true;
                }
                identity.frame = Some(frame_id(frame));
            }
            identity.last_nonce = nonce;
        }
        let reply = evaluate_helper(frame, op, &text).unwrap_or(WireReply {
            ok: false,
            copy_seen: false,
            paste_seen: false,
            has_plaintext: false,
            has_formats: false,
            oversized: false,
            text: String::new(),
        });
        let Some(mut message) = process_message_create(Some(&REPLY.into())) else {
            return true;
        };
        let Some(args) = message.argument_list() else {
            return true;
        };
        args.set_int(0, PROTOCOL_VERSION);
        args.set_string(1, Some(&marker.as_str().into()));
        args.set_string(2, Some(&transaction.to_string().as_str().into()));
        args.set_string(3, Some(&nonce.to_string().as_str().into()));
        args.set_int(4, op as i32);
        for (index, value) in [
            (5, reply.ok),
            (6, reply.copy_seen),
            (7, reply.paste_seen),
            (8, reply.has_plaintext),
            (9, reply.oversized),
            (10, reply.has_formats),
        ] {
            args.set_bool(index, i32::from(value));
        }
        args.set_string(11, Some(&reply.text.as_str().into()));
        frame.send_process_message(ProcessId::BROWSER, Some(&mut message));
        true
    }
}
fn evaluate_helper(frame: &Frame, op: WireOp, text: &str) -> Option<WireReply> {
    let context = frame.v8_context()?;
    if context.enter() != 1 {
        return None;
    }
    let output = (|| {
        let (mut result, mut exception) = (None, None);
        if context.eval(
            Some(&HELPER_FUNCTION.into()),
            Some(&"herdr-private-shuttle".into()),
            1,
            Some(&mut result),
            Some(&mut exception),
        ) != 1
        {
            return None;
        }
        let result = result?.execute_function(
            None,
            Some(&[
                v8_value_create_int(op as i32),
                v8_value_create_string(Some(&text.into())),
            ]),
        )?;
        let encoded = CefString::from(&result.string_value()).to_string();
        // JSON escaping may expand a bounded plaintext by up to 6x.
        if encoded.len() > MAX_TEXT_BYTES * 6 + 1024 {
            return None;
        }
        let value: serde_json::Value = serde_json::from_str(&encoded).ok()?;
        let text = value.get("text")?.as_str()?.to_owned();
        if !bounded(&text) {
            return None;
        }
        let reply = WireReply {
            ok: value.get("ok")?.as_bool()?,
            copy_seen: value.get("copy_seen")?.as_bool()?,
            paste_seen: value.get("paste_seen")?.as_bool()?,
            has_plaintext: value.get("has_plaintext")?.as_bool()?,
            has_formats: value.get("has_formats")?.as_bool()?,
            oversized: value.get("oversized")?.as_bool()?,
            text,
        };
        valid_reply_shape(&reply).then_some(reply)
    })();
    context.exit();
    output
}
// Fixed inert helper code ONLY. All text is a typed V8 argument. No source
// document, page binding, extension, permission override or JS interpolation.
const HELPER_FUNCTION: &str = r#"(function(op,text){
 let e=document.getElementById('herdr-shuttle');
 if(op===0){
  if(e)return JSON.stringify({ok:false,copy_seen:false,paste_seen:false,has_plaintext:false,has_formats:false,oversized:false,text:''});
  e=document.createElement('textarea');e.id='herdr-shuttle';document.body.append(e);
  const s={copy_seen:false,paste_seen:false,has_plaintext:false,has_formats:false,oversized:false,text:'',copyText:''};
  e.__herdrShuttle=s;
  e.addEventListener('copy',event=>{
   if(!event.isTrusted||!event.clipboardData)return;
   event.clipboardData.setData('text/plain',s.copyText);event.preventDefault();
   s.copy_seen=true;
  });
  e.addEventListener('paste',event=>{
   if(!event.isTrusted||!event.clipboardData)return;
   const types=Array.from(event.clipboardData.types);
   s.has_formats=types.length>0;s.has_plaintext=types.includes('text/plain');
   const value=s.has_plaintext?event.clipboardData.getData('text/plain'):'';
   // Bound the retained/returned text in UTF-16 AND UTF-8 bytes.
   s.oversized=value.length>65536||(new TextEncoder().encode(value)).length>65536;
   s.text=s.oversized?'':value;
   s.paste_seen=true;event.preventDefault();
  });
 }
 if(!e||!e.__herdrShuttle)return JSON.stringify({ok:false,copy_seen:false,paste_seen:false,has_plaintext:false,has_formats:false,oversized:false,text:''});
 const s=e.__herdrShuttle;
 if(op===1){
  s.copyText=text;s.copy_seen=false;s.paste_seen=false;s.has_plaintext=false;s.has_formats=false;s.oversized=false;s.text='';
  // Nonempty selected dummy ensures copy fires even for an EMPTY payload.
  e.value='x';e.focus();e.select();
 }
 if(op===2){s.paste_seen=false;s.has_plaintext=false;s.has_formats=false;s.oversized=false;s.text='';e.value='';e.focus();e.select();}
 return JSON.stringify({ok:true,copy_seen:s.copy_seen,paste_seen:s.paste_seen,has_plaintext:s.has_plaintext,has_formats:s.has_formats,oversized:s.oversized,text:s.text});
})"#;

#[cfg(test)]
mod tests {
    use super::*;
    fn reply(text: &str, has_plaintext: bool) -> WireReply {
        WireReply {
            ok: true,
            copy_seen: false,
            paste_seen: true,
            has_plaintext,
            // False plain + formats present models unsupported HTML/image,
            // not the distinct no-format native clear.
            has_formats: true,
            oversized: false,
            text: text.into(),
        }
    }
    fn key() -> WireKey {
        WireKey {
            browser: 7,
            frame: "frame-1".into(),
            marker: "private".into(),
            transaction: 3,
            nonce: 5,
            op: WireOp::Snapshot,
        }
    }
    fn fence_key() -> FenceKey {
        FenceKey {
            browser_id: 7,
            method_id: 11,
        }
    }
    #[test]
    fn source_fence_ids_and_parameters_are_bounded_and_command_free() {
        assert_eq!(checked_fence_method_id(0), None);
        assert_eq!(checked_fence_method_id(1), Some(1));
        assert_eq!(checked_fence_method_id(i32::MAX as u64), Some(i32::MAX));
        assert_eq!(checked_fence_method_id(i32::MAX as u64 + 1), None);
        assert_eq!(checked_fence_method_id(u64::MAX), None);
        let params: serde_json::Value = serde_json::from_str(PASTE_FENCE_PARAMS).unwrap();
        assert_eq!(params.as_object().unwrap().len(), 4);
        assert_eq!(params["type"], "keyUp");
        assert_eq!(params["key"], "Unidentified");
        assert_eq!(params["code"], "Unidentified");
        assert_eq!(params["windowsVirtualKeyCode"], 0);
        assert!(params.get("commands").is_none());
        assert!(params.get("modifiers").is_none());
        let now = Instant::now();
        let end = now + Duration::from_secs(1);
        for id in [i32::MIN, -1, 0] {
            assert!(
                FenceMailbox::new(
                    FenceKey {
                        method_id: id,
                        ..fence_key()
                    },
                    now,
                    end
                )
                .is_err()
            );
        }
        assert!(
            FenceMailbox::new(
                FenceKey {
                    browser_id: 0,
                    ..fence_key()
                },
                now,
                end
            )
            .is_err()
        );
        assert!(FenceMailbox::new(fence_key(), now, now).is_err());
    }
    #[test]
    fn source_fence_mailbox_requires_exact_identity_and_ignores_duplicates() {
        let now = Instant::now();
        let end = now + Duration::from_secs(1);
        let key = fence_key();
        let mut mailbox = FenceMailbox::new(key, now, end).unwrap();
        assert!(!mailbox.record_result(
            FenceKey {
                browser_id: 8,
                ..key
            },
            true,
            now
        ));
        assert!(!mailbox.record_result(
            FenceKey {
                method_id: 12,
                ..key
            },
            true,
            now
        ));
        assert!(!mailbox.record_result(
            FenceKey {
                method_id: 0,
                ..key
            },
            true,
            now
        ));
        assert_eq!(mailbox.poll(now), FencePoll::Pending);
        assert!(mailbox.record_result(key, true, now));
        assert!(!mailbox.record_result(key, false, now));
        assert_eq!(mailbox.poll(now), FencePoll::Acknowledged);
        mailbox.clear();
        assert!(!mailbox.record_result(key, true, now));
        assert_eq!(
            mailbox.poll(now),
            FencePoll::Failed(Failure::HelperProtocol)
        );
    }
    #[test]
    fn source_fence_accepts_on_time_arrival_but_fails_timeout_error_and_detach() {
        let now = Instant::now();
        let end = now + Duration::from_secs(1);
        let key = fence_key();
        let mut mailbox = FenceMailbox::new(key, now, end).unwrap();
        assert!(mailbox.record_result(key, true, end - Duration::from_millis(1)));
        assert_eq!(
            mailbox.poll(end + Duration::from_secs(5)),
            FencePoll::Acknowledged
        );
        let mut late = FenceMailbox::new(key, now, end).unwrap();
        assert!(!late.record_result(key, true, end));
        assert_eq!(late.poll(end), FencePoll::Failed(Failure::HelperTimeout));
        let mut failed = FenceMailbox::new(key, now, end).unwrap();
        assert!(failed.record_result(key, false, now));
        assert_eq!(failed.poll(now), FencePoll::Failed(Failure::HelperProtocol));
        let mut detached = FenceMailbox::new(key, now, end).unwrap();
        detached.record_detached(key.browser_id + 1);
        assert_eq!(detached.poll(now), FencePoll::Pending);
        assert!(detached.record_result(key, true, now));
        detached.record_detached(key.browser_id);
        assert_eq!(
            detached.poll(now),
            FencePoll::Failed(Failure::HelperProtocol)
        );
        assert!(!detached.record_result(key, true, now));
    }
    #[test]
    fn abandoned_paste_remains_draining_until_fence_then_discards_completion() {
        let now = Instant::now();
        let key = fence_key();
        let mut mailbox = FenceMailbox::new(key, now, now + Duration::from_secs(1)).unwrap();
        assert_eq!(mailbox.poll(now), FencePoll::Pending);
        // Source has been issued; losing PaneView's pending ID cannot release
        // the global slot. Coordinator retains transaction, deadline and lease.
        assert!(must_drain_on_abandonment(true, false, true));
        assert!(mailbox.record_result(key, true, now));
        assert_eq!(mailbox.poll(now), FencePoll::Acknowledged);
        assert!(
            make_completion(
                TransactionId(3),
                OwnerStamp {
                    owner: 1,
                    document_generation: 2
                },
                Operation::Paste,
                NativeFrameIdentity {
                    browser_id: key.browser_id,
                    frame_id: "source".into()
                },
                CompletionResult::PasteDispatched,
                true,
                false
            )
            .is_none()
        );
        // A missing ACK is a real transport timeout, not healthy abandonment.
        let pending = FenceMailbox::new(key, now, now + Duration::from_secs(1)).unwrap();
        assert_eq!(
            pending.poll(now + Duration::from_secs(1)),
            FencePoll::Failed(Failure::HelperTimeout)
        );
    }
    #[test]
    fn pending_async_creation_must_drain_even_without_a_browser_handle() {
        let shared = Shared::default();
        assert!(shared_drained(&shared));
        shared.creating.store(true, Ordering::Release);
        assert!(!shared_drained(&shared));
        shared.live.store(true, Ordering::Release);
        shared.creating.store(false, Ordering::Release);
        assert!(!shared_drained(&shared));
        shared.live.store(false, Ordering::Release);
        assert!(shared_drained(&shared));
    }
    #[test]
    fn healthy_no_update_is_non_success_but_reusable() {
        assert_eq!(
            observe_result(&reply("seed", true), "seed", false),
            Observation::Continue
        );
        assert_eq!(
            observe_result(&reply("seed", true), "seed", true),
            Observation::NoUpdate
        );
        assert!(!must_drain_on_abandonment(false, false, false));
        // A later operation must seed afresh, not treat this observation as an
        // engine cancellation barrier or retry Cut.
        assert_ne!(sentinel(TransactionId(1)), sentinel(TransactionId(2)));
    }
    #[test]
    fn empty_plaintext_is_a_valid_result_and_differs_from_absence() {
        assert_eq!(
            observe_result(&reply("", true), "seed", false),
            Observation::Text
        );
        assert_eq!(
            observe_result(&reply("", false), "seed", false),
            Observation::Continue
        );
        assert_eq!(
            observe_result(&reply("", false), "seed", true),
            Observation::NoPlaintext
        );
        assert!(bounded(""));
    }
    #[test]
    fn no_format_clear_waits_for_a_healthy_window_end() {
        let mut cleared = reply("", false);
        cleared.has_formats = false;
        assert!(valid_reply_shape(&cleared));
        assert_eq!(
            observe_result(&cleared, "verified plain sentinel", false),
            Observation::Continue
        );
        assert_eq!(
            observe_result(&cleared, "verified plain sentinel", true),
            Observation::Text
        );
        assert!(cleared.text.is_empty()); // Existing Text branch yields CopiedText("").
        // An early no-format sample does NOT win over a subsequent committed
        // nonempty payload. No source command retry or synthetic event is used.
        assert_eq!(
            observe_result(
                &reply("committed text", true),
                "verified plain sentinel",
                false
            ),
            Observation::Text
        );
        assert_eq!(
            observe_result(
                &reply("verified plain sentinel", true),
                "verified plain sentinel",
                true
            ),
            Observation::NoUpdate
        );
        assert_eq!(
            observe_result(&reply("", false), "verified plain sentinel", true),
            Observation::NoPlaintext
        );
        // Default/unsettled state is not evidence of an actual native clear.
        cleared.paste_seen = false;
        assert_eq!(
            observe_result(&cleared, "verified plain sentinel", true),
            Observation::Continue
        );
        cleared.paste_seen = true;
        cleared.ok = false;
        assert_eq!(
            observe_result(&cleared, "verified plain sentinel", true),
            Observation::NoPlaintext
        );
    }
    #[test]
    fn empty_seed_accepts_observed_clear_but_not_unsupported_or_unsettled_data() {
        let seeded = |text: &str, plain| {
            let mut state = reply(text, plain);
            state.copy_seen = true;
            state
        };
        let mut cleared = seeded("", false);
        cleared.has_formats = false;
        assert!(seed_verified(&cleared, ""));
        assert!(!seed_verified(&cleared, "nonempty sentinel"));
        assert!(seed_verified(&seeded("", true), "")); // A present empty plain format also works.
        assert!(seed_verified(&seeded("seed", true), "seed"));
        assert!(!seed_verified(&seeded("", false), "")); // HTML/image-only is NOT empty seed.
        assert!(!seed_verified(&seeded("different", true), "seed"));
        cleared.copy_seen = false;
        assert!(!seed_verified(&cleared, ""));
        cleared.copy_seen = true;
        cleared.paste_seen = false;
        assert!(!seed_verified(&cleared, ""));
        cleared.paste_seen = true;
        cleared.ok = false;
        assert!(!seed_verified(&cleared, ""));
        cleared.ok = true;
        cleared.oversized = true;
        assert!(!seed_verified(&cleared, ""));
    }
    #[test]
    fn inconsistent_format_flags_cannot_export_stale_text_as_a_clear() {
        let mut malformed = reply("stale data", false);
        malformed.has_formats = false;
        assert!(!valid_reply_shape(&malformed));
        assert_eq!(
            observe_result(&malformed, "seed", true),
            Observation::NoPlaintext
        );
        malformed = reply("", true);
        malformed.has_formats = false; // Present plain requires formats present.
        assert!(!valid_reply_shape(&malformed));
        malformed = reply("", false);
        malformed.has_formats = false;
        malformed.oversized = true; // No-format clear cannot advertise oversized plaintext.
        assert!(!valid_reply_shape(&malformed));
    }
    #[test]
    fn abandoned_results_never_produce_an_exportable_completion() {
        let owner = OwnerStamp {
            owner: 1,
            document_generation: 2,
        };
        for result in [
            CompletionResult::CopiedText("text".into()),
            CompletionResult::CopiedText(String::new()),
            CompletionResult::NoUpdateObserved,
            CompletionResult::Failed(Failure::HelperTimeout),
        ] {
            assert!(
                make_completion(
                    TransactionId(3),
                    owner,
                    Operation::Cut,
                    NativeFrameIdentity {
                        browser_id: 7,
                        frame_id: "focused-frame".into()
                    },
                    result,
                    true,
                    false
                )
                .is_none()
            );
        }
        let completion = make_completion(
            TransactionId(4),
            owner,
            Operation::Copy,
            NativeFrameIdentity {
                browser_id: 7,
                frame_id: "focused-frame".into(),
            },
            CompletionResult::CopiedText(String::new()),
            false,
            false,
        )
        .unwrap();
        assert!(matches!(completion.result, CompletionResult::CopiedText(text) if text.is_empty()));
        assert!(!completion.coordinator_poisoned);
    }
    #[test]
    fn bounds_are_utf8_bytes_not_characters() {
        assert!(bounded(&"x".repeat(MAX_TEXT_BYTES)));
        assert!(!bounded(&"x".repeat(MAX_TEXT_BYTES + 1)));
        assert!(!bounded(&"🦀".repeat(MAX_TEXT_BYTES / 4 + 1)));
        assert_eq!(decimal("0", false), None);
        assert_eq!(decimal("0", true), Some(0));
        for bad in [
            "",
            "-1",
            "+1",
            "01",
            " 1",
            "1 ",
            "1.0",
            "18446744073709551616",
        ] {
            assert_eq!(decimal(bad, false), None);
        }
    }
    #[test]
    fn nonce_identity_and_deadline_are_exact() {
        let now = Instant::now();
        let expected = key();
        let pending = Pending {
            key: expected.clone(),
            deadline: now + Duration::from_secs(1),
        };
        assert!(accepts(&pending, &expected, now));
        assert!(!accepts(&pending, &expected, pending.deadline));
        let mut wrong = expected.clone();
        wrong.nonce += 1;
        assert!(!accepts(&pending, &wrong, now));
        wrong = expected.clone();
        wrong.browser += 1;
        assert!(!accepts(&pending, &wrong, now));
        wrong = expected.clone();
        wrong.frame.push('2');
        assert!(!accepts(&pending, &wrong, now));
        wrong = expected.clone();
        wrong.transaction += 1;
        assert!(!accepts(&pending, &wrong, now));
        wrong = expected.clone();
        wrong.marker.push('!');
        assert!(!accepts(&pending, &wrong, now));
        wrong = expected;
        wrong.op = WireOp::ArmCopy;
        assert!(!accepts(&pending, &wrong, now));
    }
    #[test]
    fn abandonment_drains_instead_of_poisoning_or_reissuing_source() {
        assert!(must_drain_on_abandonment(true, false, false));
        assert!(must_drain_on_abandonment(false, true, false));
        assert!(must_drain_on_abandonment(false, false, true));
        assert!(!must_drain_on_abandonment(false, false, false));
        // A healthy final sentinel read remains the same non-poisoning policy
        // when the caller has abandoned the result; only export is suppressed.
        assert_eq!(
            observe_result(&reply("seed", true), "seed", true),
            Observation::NoUpdate
        );
    }
    #[test]
    fn normal_competing_writer_is_observed_not_cryptographically_attributed() {
        assert_eq!(
            observe_result(&reply("ordinary gesture writer", true), "seed", true),
            Observation::Text
        );
        // Required integration semantics: shared clipboard behavior, not proof
        // that the captured source exclusively wrote the observed plaintext.
    }
}
