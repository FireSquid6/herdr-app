use super::*;

impl Panel {
    pub(super) fn start(
        config: Config,
        force: bool,
        previous: Option<mpsc::Receiver<()>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Config { workspace, machine, viewer, group, attached, visible:initial_visible } = config;
        let (finished_tx, finished) = mpsc::channel();
        let mut initial = Snapshot::default();
        if let Some(group) = &group {
            let mut metadata = group.saved.restore_params();
            metadata["group"] = json!(group.id);
            for tab in metadata["tabs"].as_array_mut().unwrap() { tab["target"] = Value::Null; }
            initial.tabs(metadata);
        }
        if !attached { initial.phase = Phase::Detached; }
        let shared = Arc::new(Mutex::new(initial));
        let visible = Arc::new(AtomicBool::new(initial_visible));
        let closed = Arc::new(AtomicBool::new(false));
        let worker_closed = closed.clone();
        let (commands, rx) = mpsc::sync_channel::<Command>(128);
        let state = shared.clone();
        let shown = visible.clone();
        let ws = workspace.clone();
        let host = machine.clone();
        let connection = viewer.clone();
        let worker_commands = commands.clone();
        let worker_group = group.clone();
        if attached { std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                if let Some(previous) = previous {
                    if matches!(
                        previous.recv_timeout(Duration::from_secs(30)),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ) {
                        bail!("Previous viewer has not disconnected; retry explicitly");
                    }
                }
                if !host.is_empty() {
                    bail!(
                        "Remote browser needs a dedicated authenticated viewer connection to {host}; local fallback is forbidden"
                    );
                }
                if worker_closed.load(Ordering::SeqCst) { return Ok(()); }
                let core = connection.connect()?;
                let negotiated = core.call("browser.negotiate", json!({"revision":2,"client":"herdr-native"}))
                    .map_err(|e| anyhow::anyhow!("Workspace browser unsupported: enable HC_BROWSER_CHROME on a revision-2 core ({e:#})"))?;
                if negotiated["revision"] != 2 {
                    bail!("Unsupported browser wire revision");
                }
                if worker_group.is_some() && !negotiated["capabilities"].as_array()
                    .is_some_and(|caps| caps.iter().any(|cap| cap == "pane_groups_v1")) {
                    bail!("Workspace browser does not support pane-owned groups");
                }
                let running = core.call_timeout(
                    "browser.ensure",
                    json!({"workspace":ws}),
                    Duration::from_secs(60),
                )?;
                if worker_closed.load(Ordering::SeqCst) { return Ok(()); }
                let instance = running["instance"].as_str().filter(|id| !id.is_empty() && id.len() <= 128)
                    .ok_or_else(|| anyhow::anyhow!("Missing browser instance identity"))?.to_owned();
                let acquired = core.call(
                    "browser.acquire",
                    json!({"workspace":ws,"force":force,"confirmed":force}),
                )?;
                let generation = acquired["generation"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing controller generation"))?
                    .to_owned();
                let controller_lease = json!({"workspace":ws,"generation":generation});
                let mut lease = controller_lease.clone();
                if let Some(group) = &worker_group { lease["group"] = json!(group.id); }
                if worker_closed.load(Ordering::SeqCst) {
                    return Ok(());
                }
                let tabs = if let Some(group) = &worker_group {
                    let mut params = lease.clone();
                    params["restore"] = group.saved.restore_params();
                    params["reset_frames"] = json!(true);
                    params["visible"] = json!(shown.load(Ordering::Relaxed));
                    core.call("browser.group.ensure", params)?
                } else { core.call("browser.tabs", lease.clone())? };
                {
                    let mut s = state.lock().unwrap();
                    if worker_closed.load(Ordering::SeqCst) {
                        drop(s);
                        return Ok(());
                    }
                    s.clipboard_capable = negotiated["capabilities"].as_array()
                        .is_some_and(|caps| caps.iter().any(|cap| cap == "clipboard_text_v1"));
                    s.context_capable = negotiated["capabilities"].as_array()
                        .is_some_and(|caps| caps.iter().any(|cap| cap == "context_menu_v1"));
                    s.generation = generation.clone();
                    s.instance = instance.clone();
                    s.browser_pid = running["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()).unwrap_or(0);
                    s.ready = true;
                    s.phase = Phase::Running;
                    s.tabs(tabs);
                }
                let frame_core = core.clone();
                let frame_state = state.clone();
                let frame_lease = lease.clone();
                let heartbeat_lease = controller_lease;
                let frame_host = host.clone();
                let frame_ws = ws.clone();
                let follow_commands = worker_commands;
                // Exactly one capture/decode in flight per surface. Control RPCs run
                // on the other thread and do not wait for image decompression.
                let frames = std::thread::spawn(move || {
                    let mut acknowledgements =
                        std::collections::HashMap::<(String, usize, u64), SurfaceState>::new();
                    let mut heartbeat = Instant::now();
                    let mut metadata = Instant::now();
                    let mut group_visibility = None;
                    loop {
                        if !frame_state.lock().unwrap().ready {
                            break;
                        }
                        if heartbeat.elapsed() > Duration::from_secs(5) {
                            match frame_core.call("browser.heartbeat", heartbeat_lease.clone()) {
                                Ok(reply) => {
                                    let mut s = frame_state.lock().unwrap();
                                    if !s.ready { break; }
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
                                    s.observation_failed(Phase::Revoked, format!("Controller disconnected/revoked: {e:#}"));
                                    break;
                                }
                            }
                            heartbeat = Instant::now();
                        }
                        let visible = shown.load(Ordering::Relaxed);
                        if frame_lease["group"].is_string() && group_visibility != Some(visible) {
                            let mut params = frame_lease.clone();
                            params["visible"] = json!(visible);
                            // Warm ensure retains live membership; becoming visible
                            // instantiates only an unloaded selected entry.
                            match frame_core.call("browser.group.ensure", params) {
                                Ok(tabs) => {
                                    let mut state = frame_state.lock().unwrap();
                                    if !state.ready { break; }
                                    state.tabs(tabs);
                                    group_visibility = Some(visible);
                                }
                                Err(error) => {
                                    let mut state = frame_state.lock().unwrap();
                                    state.observation_failed(Phase::Failed, format!("Browser group visibility failed: {error:#}"));
                                    break;
                                }
                            }
                        }
                        if !visible {
                            std::thread::park_timeout(Duration::from_millis(100));
                            continue;
                        }
                        if metadata.elapsed() > Duration::from_secs(1) {
                            let mut params = frame_lease.clone();
                            let method = if params["group"].is_string() {
                                params["visible"] = json!(true);
                                "browser.group.ensure"
                            } else { "browser.tabs" };
                            match frame_core.call(method, params) {
                                Ok(tabs) => {
                                    let mut s = frame_state.lock().unwrap();
                                    if s.ready && s.generation == generation { s.tabs(tabs); }
                                }
                                Err(error) if method == "browser.group.ensure" => {
                                    let mut s = frame_state.lock().unwrap();
                                    s.observation_failed(Phase::Failed, format!("Group observation failed; explicit attach required: {error:#}"));
                                    break;
                                }
                                Err(_) => {}
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
                                    && s.owns_target(id)
                            });
                        }
                        if target.is_empty() {
                            std::thread::park_timeout(Duration::from_millis(100));
                            continue;
                        }
                        {
                            let mut s = frame_state.lock().unwrap();
                            if s.needs_follow(&target) {
                                s.tools_requested_target = target.clone();
                                if follow_commands
                                    .try_send(Command {
                                        method: "browser.devtools.open",
                                        params: json!({"target":target}),
                                        follow: true,
                                        clipboard: None,
                                        context: None,
                                    })
                                    .is_err()
                                {
                                    s.tools_requested_target.clear();
                                    s.error = "DevTools follow queue full".into();
                                    s.revision += 1;
                                }
                            }
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
                                    if matches!(
                                        crate::hc::error_kind(&e),
                                        Some("not_owner" | "stale_generation")
                                    ) {
                                        s.phase = Phase::Revoked;
                                        s.ready = false;
                                        s.page = None;
                                        s.devtools = None;
                                    }
                                    s.error = format!("Frame: {e:#}");
                                    s.revision += 1;
                                }
                            }
                        }
                        std::thread::park_timeout(Duration::from_millis(100));
                    }
                });
                for mut cmd in rx.iter() {
                    if let Some(request) = cmd.clipboard.take() {
                        clipboard::execute(&core, &state, request, cmd.method, cmd.params);
                        continue;
                    }
                    if let Some(request) = cmd.context.take() {
                        context_menu::execute(&core, &state, request, cmd.params);
                        continue;
                    }
                    // Explicit reconnect/stop cancels queued work, not merely UI
                    // commits. Never execute old target input after replacement.
                    if !state.lock().unwrap().ready {
                        break;
                    }
                    if cmd.follow {
                        let mut s = state.lock().unwrap();
                        if !s.accept_follow(cmd.params["target"].as_str().unwrap_or_default()) {
                            continue;
                        }
                    }
                    let mut params = lease.clone();
                    if cmd.method == "browser.stop" { params.as_object_mut().unwrap().remove("group"); }
                    if let Some(values) = cmd.params.as_object() {
                        for (k, v) in values {
                            params[k] = v.clone();
                        }
                    }
                    if matches!(cmd.method, "browser.devtools.open" | "browser.devtools.close") {
                        if let Some(tab) = params["tab"].as_str() {
                            let target = state.lock().unwrap().tabs.iter().find(|entry| entry["id"] == tab)
                                .map(Snapshot::entry_target).unwrap_or_default().to_owned();
                            if target.is_empty() {
                                let mut snapshot = state.lock().unwrap();
                                snapshot.error = "DevTools target is not loaded".into();
                                snapshot.revision += 1;
                                continue;
                            }
                            params.as_object_mut().unwrap().remove("tab");
                            params["target"] = target.into();
                        }
                    }
                    if cmd.method == "browser.devtools.open" {
                        let previous = state.lock().unwrap().tools_target.clone();
                        if !previous.is_empty() && params["target"] != previous {
                            let mut close = lease.clone();
                            close["target"] = previous.into();
                            if let Err(e) = core.call("browser.devtools.close", close) {
                                if crate::hc::error_kind(&e) != Some("not_found") {
                                    let mut s = state.lock().unwrap();
                                    s.error = format!("DevTools close failed: {e:#}");
                                    s.revision += 1;
                                    continue;
                                }
                            }
                        }
                    }
                    if cmd.method == "browser.devtools.close" {
                        let previous = state.lock().unwrap().tools_target.clone();
                        if !previous.is_empty() {
                            params["target"] = previous.into();
                        }
                    }
                    let requested_target = params["target"].as_str().unwrap_or_default().to_owned();
                    match core.call(cmd.method, params) {
                        Ok(reply) => {
                            if cmd.method == "browser.stop" && reply["state"] != "stopped" {
                                let mut s = state.lock().unwrap();
                                s.error = "Browser exit was not observed; session remains attached"
                                    .into();
                                s.revision += 1;
                                continue;
                            }
                            if cmd.method == "browser.stop" {
                                connection.observed_stop(&instance);
                                state.lock().unwrap().observed_stop();
                                break;
                            }
                            {
                                let mut s = state.lock().unwrap();
                                if s.error.starts_with(cmd.method) {
                                    s.error.clear();
                                    s.revision += 1;
                                }
                            }
                            if cmd.method == "browser.devtools.open" {
                                let mut s = state.lock().unwrap();
                                s.tools_opened(requested_target, cmd.follow);
                            }
                            if cmd.method == "browser.devtools.close" {
                                let mut s = state.lock().unwrap();
                                s.tools_open = false;
                                s.tools_wanted = false;
                                s.tools_target.clear();
                                s.tools_requested_target.clear();
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
                                s.phase = Phase::Revoked;
                                s.ready = false;
                                s.page = None;
                                s.devtools = None;
                            }
                            if lease["group"].is_string() && cmd.method == "browser.tab.select"
                                && !matches!(crate::hc::error_kind(&e), Some("invalid_params" | "not_found")) {
                                // A lazy native target may have been created before
                                // transport failure. Never auto-repeat materialization.
                                s.phase = Phase::Failed;
                                s.ready = false;
                            }
                            s.error = format!("{}: {e:#}", cmd.method);
                            s.revision += 1;
                        }
                    }
                }
                {
                    state.lock().unwrap().ready = false;
                }
                // Dropping unexecuted requests releases their leases; the UI
                // tick reports abandonment without ever retargeting them.
                for _ in rx.try_iter() {}
                // Other panes may share this controller identity. Connection
                // EOF revokes only when the last live runtime drops its Arc.
                let _ = frames.join();
                Ok(())
            })();
            if let Err(e) = result {
                let mut s = state.lock().unwrap();
                s.phase = if crate::hc::error_kind(&e) == Some("owned") {
                    Phase::Blocked
                } else {
                    Phase::Failed
                };
                s.error = format!("{e:#}");
                s.revision += 1;
            }
            let _ = finished_tx.send(());
        }); } else {
            // Saved slots are inert until an explicit attach. No connection,
            // worker or Chromium process is created for this renderer.
            drop(rx);
            let _ = finished_tx.send(());
        }
        let refresh = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| {
                        {
                            let mut state = this.shared.lock().unwrap();
                            if this.viewer.was_stopped(&state.instance) { state.observed_stop(); }
                        }
                        this.complete_clipboard(window, cx);
                        this.complete_context(window, cx);
                        let revision = this.shared.lock().unwrap().revision;
                        if revision != this.seen {
                            this.seen = revision;
                            this.refresh_chrome(cx);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let owner = cx.entity().downgrade();
        let page_view = cx.new(|_| super::chrome::NativePage(owner));
        let focus = cx.focus_handle();
        let clipboard_focus = vec![
            cx.on_blur(&focus, window, |this, _, _| this.invalidate_clipboard()),
            cx.observe_window_activation(window, |this, window, _| {
                if !window.is_window_active() { this.invalidate_clipboard(); }
            }),
        ];
        Self {
            pane: String::new(),
            page_view,
            chrome: None,
            chrome_copy: 0,
            chrome_runtime: None,
            workspace,
            machine,
            viewer,
            group,
            shared,
            visible,
            closed,
            pending_clipboard: None,
            context_menu: None,
            context_pressed: None,
            _clipboard_focus: clipboard_focus,
            commands,
            focus,
            page_bounds: Rc::default(),
            tools_bounds: Rc::default(),
            address: "about:blank".into(),
            editing_address: false,
            tools_focus: false,
            seen: 0,
            last_target: String::new(),
            last_tools_epoch: 0,
            pressed: Default::default(),
            finished: Some(finished),
            confirm_takeover: false,
            confirm_stop: false,
            _refresh: refresh,
        }
    }
}
