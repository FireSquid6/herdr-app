//! Optional browser views belong to model panes, not to the current tab/layout.
use super::*;

#[cfg(all(feature = "browser", target_os = "linux"))]
use crate::browser::{BrowserEvent, BrowserOptions, BrowserPane};

#[cfg(all(feature = "browser", target_os = "linux"))]
pub(super) struct BrowserSlot {
    workspace: String,
    machine: Option<String>,
    url: String,
    pub view: Entity<BrowserPane>,
    _events: Subscription,
}

impl AppWindow {
    pub(super) fn discard_stale_browsers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(all(feature = "browser", target_os = "linux"))]
        {
            let gone = self.browsers.iter().filter(|(pane, slot)| !slot.belongs_to(&self.model, pane)).map(|(pane, _)| pane.clone()).collect::<Vec<_>>();
            self.close_browsers(gone, window, cx);
        }
        #[cfg(not(all(feature = "browser", target_os = "linux")))]
        {
            let owned = self.model.tabs().into_iter().flat_map(|tab| tab.root.walk()).filter(|node| matches!(node.content(), Some(Content::Browser { .. }))).map(|node| node.id.clone()).collect();
            crate::browser_chrome::retain_states(&owned);
            let _ = (window, cx);
        }
    }

    /// Close removed/retargeted pages *before* installing a new machine document.
    pub(super) fn discard_replaced_browsers(&mut self, document: &crate::mirror::Document, machine: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(all(feature = "browser", target_os = "linux"))]
        {
            let mut next = Model::default();
            document.replace(&mut next, machine);
            let gone = self.browsers.iter().filter(|(pane, slot)| slot.machine.as_deref() == machine && !slot.belongs_to(&next, pane)).map(|(pane, _)| pane.clone()).collect::<Vec<_>>();
            self.close_browsers(gone, window, cx);
        }
        #[cfg(not(all(feature = "browser", target_os = "linux")))]
        let _ = (document, machine, window, cx);
    }

    #[cfg(all(feature = "browser", target_os = "linux"))]
    fn close_browsers(&mut self, panes: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        for pane in panes {
            if let Some(slot) = self.browsers.remove(&pane) {
                // Frame references may retain the entity, so do not rely on Drop.
                slot.view.update(cx, |view, cx| view.close(window, cx));
            }
        }
    }

    /// Reconcile ownership even for hidden pages; instantiate only visible panes.
    /// Explicitly close stale pages before any replacement is created.
    pub(super) fn prepare_browsers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(all(feature = "browser", target_os = "linux"))]
        {
            self.discard_stale_browsers(window, cx);
            let visible: Vec<_> = if self.shows_tab() {
                self.model.active_space().into_iter().flat_map(|space| {
                    space.active_tab().into_iter().flat_map(move |tab| tab.shown().visible_blocks().into_iter().filter_map(move |node| {
                        let Content::Browser { url, tabs } = node.content()? else { return None };
                        Some((node.id.clone(), space.id.clone(), space.machine.clone(), url.clone(), tabs.clone()))
                    }))
                }).collect()
            } else { Vec::new() };
            for (pane, slot) in &self.browsers {
                let shown = visible.iter().any(|(id, _, _, _, _)| id == pane);
                slot.view.update(cx, |view, cx| {
                    let copy = self.copies.get(&format!("{pane}#browser"));
                    view.set_chrome(copy.and_then(|c| c.root.clone()), copy.map_or(0, |c| c.id), cx);
                    if !shown { view.set_visible(false, window, cx); }
                    if let Some(Content::Browser { url, tabs }) = self.model.tab_of(pane).and_then(|t|t.root.find(pane)).and_then(|n|n.content()) {
                        view.sync_tabs(tabs.clone(), url, window, cx);
                    }
                    view.set_visible(shown, window, cx);
                });
            }
            for (pane, workspace, machine, url, tabs) in visible {
                if let Some(slot) = self.browsers.get_mut(&pane) {
                    slot.url = url.clone();
                    continue;
                }
                let options = BrowserOptions { workspace: workspace.clone(), pane: pane.clone(), url: url.clone(), tabs, profile_root: mods::app_dir().join("browser-profiles"), remote: machine.is_some() };
                let Some(runtime) = self.runtime.clone() else { continue };
                let view = cx.new(|cx| BrowserPane::new(options, runtime, window, cx));
                if let Some(copy) = self.copies.get(&format!("{pane}#browser")) {
                    view.update(cx, |view, cx| view.set_chrome(copy.root.clone(), copy.id, cx));
                }
                let target = pane.clone();
                let owner = workspace.clone();
                let on = machine.clone();
                let events = cx.subscribe_in(&view, window, move |this, source, event: &BrowserEvent, window, cx| {
                    // A queued event from an old owner/session must never update its replacement.
                    if !this.browsers.get(&target).is_some_and(|s| s.workspace == owner && s.machine == on && s.view == *source) { return; }
                    let BrowserEvent::Navigated { tab: browser_tab, url } = event;
                    if let Some(tab) = this.model.tab_of(&target).map(|t| t.id.clone())
                        && let Some(node) = this.model.tab_mut(&tab).and_then(|t| t.root.find_mut(&target))
                        && let Kind::Block { content: Content::Browser { url: saved, tabs } } = &mut node.kind
                    {
                        tabs.ensure(saved);
                        let Some(item) = tabs.items.iter_mut().find(|t| &t.id == browser_tab) else { return };
                        item.url = url.clone();
                        if tabs.selected == *browser_tab {
                            *saved = url.clone();
                            if let Some(slot) = this.browsers.get_mut(&target) { slot.url = url.clone(); }
                        }
                        this.save();
                        this.publish(cx);
                        this.flush_documents(false, window, cx);
                        cx.notify();
                    }
                });
                if self.model.focused_pane().as_deref() == Some(&pane) && self.focus.is_focused(window)
                    && !self.overlays.iter().any(|o| o.kind.modal_keys()) {
                    view.read(cx).focus_handle(cx).focus(window, cx);
                }
                self.browsers.insert(pane, BrowserSlot { workspace, machine, url, view, _events: events });
            }
        }
        #[cfg(not(all(feature = "browser", target_os = "linux")))]
        let _ = (window, cx);
    }

    pub(super) fn browser_chrome_instance(&self, pane: &str, url: &str) -> (String, Instance) {
        #[cfg(not(all(feature = "browser", target_os = "linux")))]
        let mut tabs = self.model.tab_of(pane).and_then(|t|t.root.find(pane)).and_then(|n| match n.content() { Some(Content::Browser { tabs, .. }) => Some(tabs.clone()), _ => None }).unwrap_or_default();
        #[cfg(not(all(feature = "browser", target_os = "linux")))]
        tabs.ensure(url);
        #[cfg(not(all(feature = "browser", target_os = "linux")))]
        crate::browser_chrome::publish_state(pane, json!({
            "selected_tab":tabs.selected, "active_tab":tabs.selected,
            "tabs":tabs.items.iter().map(|t|json!({"id":t.id,"url":t.url,"title":t.url,"favicon":null,"loading":false,"loaded":false,"visible":false,"browser_id":null,"frame_copies":0,"frame_uploads":0})).collect::<Vec<_>>(),
            "pane": pane, "workspace": self.model.space_of(pane).map(|s| s.id.as_str()).unwrap_or(""),
            "url": url, "loading": false, "can_back": false, "can_forward": false,
            "remote": self.model.space_of(pane).is_some_and(|s| s.machine.is_some()),
            "closed": false, "available": false,
            "error": "Browser unavailable: requires the optional Linux browser build and runtime.",
        }));
        let _ = url;
        (format!("{pane}#browser"), Instance { name: self.person.browser_chrome.clone(), props: json!({"pane":pane}) })
    }

    pub(super) fn render_browser(&self, pane: &str, cx: &mut Context<Self>) -> AnyElement {
        let pane = pane.to_owned();
        let chrome_slot = format!("{pane}#browser");
        // The chrome slot is synthetic: render_slot cannot select its owning
        // model pane. Keep selection outside *all* branches, including errors
        // and feature-off placeholders, and above controls that consume clicks.
        let target = pane.clone();
        let frame = div().size_full()
            // Native BrowserPane mounts ShellRoot directly rather than through
            // render_slot. Its address editor and page need the ordinary mod
            // context too, using the copy actually loaded, not a stale setting.
            .when_some(self.copies.get(&chrome_slot), |el, copy| el.key_context(format!("Mod mod = {}", copy.inst.name).as_str()))
            .capture_any_mouse_down(cx.listener(move |this, _, window, cx| {
                if this.focused_pane.as_deref() != Some(target.as_str()) {
                    // A blank/error view has no focusable child. Release the
                    // neighboring terminal so render cannot reselect it; an
                    // input or native page clicked below takes focus normally.
                    this.focus.focus(window, cx);
                }
                this.model.focus_pane(&target);
                this.focused_pane = Some(target.clone());
                this.synced_model_pane = this.model.focused_pane();
                this.focused_block = None;
                this.publish(cx);
                this.save();
                cx.notify();
            }));
        let copy = self.copies.get(&chrome_slot);
        if copy.is_some_and(|copy| copy.root.is_none()) {
            return frame.child(self.render_slot(&chrome_slot, cx)).into_any_element();
        }
        #[cfg(all(feature = "browser", target_os = "linux"))]
        if let Some(slot) = self.browsers.get(&pane) {
            return frame.child(slot.view.clone()).into_any_element();
        }
        if copy.is_some() { return frame.child(self.render_slot(&chrome_slot, cx)).into_any_element(); }
        frame.child("Browser unavailable: requires the optional Linux browser build and runtime.").into_any_element()
    }

    fn browser_tab_command(&mut self, name: &str, args: &Value, window: &mut Window, cx: &mut Context<Self>) -> Result<Value, String> {
        for key in ["tab", "url"] {
            if args.get(key).is_some_and(|v|!v.is_null() && !v.is_string()) { return Err(format!("{name} {key} must be a string")); }
        }
        let pane = Self::owned(args, "pane").or_else(|| self.model.focused_pane()).ok_or("no browser pane focused")?;
        let space = self.model.space_of(&pane).ok_or("no such browser pane")?;
        if name == "browser.tab.new" {
            if space.machine.is_some() { return Err("Remote browser route unavailable: direct networking is blocked".into()); }
            #[cfg(not(all(feature = "browser", target_os = "linux")))]
            return Err("Browser unavailable: new pages require the optional Linux browser build and runtime".into());
        }
        let machine = space.machine.clone();
        self.require_document(machine.as_deref())?;
        let tab = self.model.tab_of(&pane).ok_or("no such browser pane")?.id.clone();
        // Only this exact browser-only final workspace may skip the terminal
        // confirmation. Never authorize closing a space containing any terminal.
        let empty_browser_space = (space.tabs.len() == 1
            && space.tabs[0].root.panes().len() == 1
            && space.tabs[0].root.terminals().is_empty()
            && space.tabs[0].root.walk().iter().all(|node|node.content().is_none_or(|content|matches!(content, Content::Browser { .. }))))
            .then(|| space.id.clone());
        let id = self.model.fresh_id("bt");
        let node = self.model.tab_mut(&tab).and_then(|t| t.root.find_mut(&pane)).ok_or("no such browser pane")?;
        let closable = node.closable();
        let Kind::Block { content: Content::Browser { url, tabs } } = &mut node.kind else { return Err("target is not a browser pane".into()) };
        let mut next = tabs.clone();
        next.ensure(url);
        let target = Self::arg(args, "tab").unwrap_or(&next.selected).to_owned();
        let ok = match name {
            "browser.tab.new" => {
                let input = Self::arg(args, "url").unwrap_or(&self.person.browser_url);
                next.insert(id.clone(), settings::browser::target(input, &self.person.browser_search)); true
            }
            "browser.tab.select" => next.select(&target),
            "browser.tab.close" => next.close(&target),
            "browser.tab.move" => {
                let index = args.get("index").and_then(Value::as_u64).ok_or("browser.tab.move index must be a nonnegative integer")?;
                next.move_to(&target, index as usize)
            }
            _ => false,
        };
        if !ok { return Err("no such browser tab".into()); }
        if next.items.is_empty() && !closable { return Err("this browser pane is not closable".into()); }
        if next.items.is_empty() {
            // Keep the original durable list until the owning pane is gone.
            // A failed nested close must neither regenerate an initial tab nor
            // be overwritten by this command's successful-looking result.
            if let Some(space) = empty_browser_space {
                self.dispatch("space.close", &json!({"id":space,"confirmed":true}), false, window, cx);
            } else {
                self.dispatch("pane.close", &json!({"pane":pane}), false, window, cx);
            }
            if let Some(error) = self.reply.take().and_then(|v|v.get("error").and_then(Value::as_str).map(str::to_owned)) {
                return Err(error);
            }
            if self.model.tab_of(&pane).is_some() { return Err("browser pane was not closed".into()); }
        } else {
            *url = next.url().unwrap().into();
            *tabs = next;
            #[cfg(all(feature = "browser", target_os = "linux"))]
            self.reconcile_browser_selection(&pane, window, cx);
        }
        Ok(json!({"pane":pane,"tab":if name == "browser.tab.new" { id } else { target }}))
    }

    /// Resolve model selection before any selected-page operation, without
    /// creating a browser or making a hidden pane visible. Context-menu/async
    /// entrypoints must use this before capturing their native page identity.
    #[cfg(all(feature = "browser", target_os = "linux"))]
    pub(super) fn reconcile_browser_selection(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<BrowserPane>> {
        self.discard_stale_browsers(window, cx);
        let Content::Browser { url, tabs } = self.model.tab_of(pane)?.root.find(pane)?.content()? else { return None };
        let view = self.browsers.get(pane)?.view.clone();
        view.update(cx, |view, cx| view.sync_tabs(tabs.clone(), url, window, cx));
        Some(view)
    }

    pub(super) fn browser_command(&mut self, name: &str, args: &Value, window: &mut Window, cx: &mut Context<Self>) -> Result<Value, String> {
        if name.starts_with("browser.tab.") { return self.browser_tab_command(name, args, window, cx); }
        #[cfg(not(all(feature = "browser", target_os = "linux")))]
        { let _ = (name, args, window, cx); Err("Browser unavailable: requires the optional Linux browser build and runtime".into()) }
        #[cfg(all(feature = "browser", target_os = "linux"))]
        {
            if name == "browser.new" {
                let place = Self::arg(args, "place").unwrap_or("tab");
                if !matches!(place, "tab" | "right" | "down") { return Err("browser.new place is tab, right or down".into()); }
                let url = match args.get("url") {
                    None | Some(Value::Null) => self.person.browser_url.as_str(),
                    Some(Value::String(url)) => url,
                    _ => return Err("browser.new url must be a string".into()),
                };
                let url = settings::browser::target(url, &self.person.browser_search);
                let target = Self::owned(args, "pane").or_else(|| self.model.focused_pane());
                let space = if place == "tab" {
                    Self::owned(args, "space").or_else(|| self.model.active.clone())
                } else {
                    if Self::arg(args, "space").is_some() { return Err("browser.new space is only valid with place tab".into()); }
                    target.as_deref().and_then(|p| self.model.space_of(p)).map(|s| s.id.clone())
                }.ok_or("no space yet: make one first (space.new)")?;
                let owner = self.model.space(&space).ok_or_else(|| format!("no space {space:?}"))?;
                // Remote browser creation is rejected; restored remote documents render a blocked view.
                if owner.machine.is_some() { return Err("Remote browser route unavailable: direct networking is blocked".into()); }
                self.require_document(None)?;
                let pane = self.model.fresh_id("p");
                let node = Node::block(pane.clone(), Content::Browser { url, tabs: Default::default() });
                if place == "tab" {
                    let tab = self.model.fresh_id("tb");
                    self.model.add_tab(&space, Tab::with_pane(&tab, "Browser", node));
                    self.model.active = Some(space.clone());
                    return Ok(json!({"space":space,"tab":tab,"pane":pane}));
                }
                let target = target.ok_or("no pane to split")?;
                self.model.split_with(&target, if place == "right" { Axis::Row } else { Axis::Column }, node);
                return Ok(json!({"pane":pane}));
            }
            let pane = Self::owned(args, "pane").or_else(|| self.model.focused_pane()).ok_or("no browser pane focused")?;
            let space = self.model.space_of(&pane).ok_or("no such browser pane")?;
            if space.machine.is_some() { return Err("Remote browser route unavailable: direct networking is blocked".into()); }
            // sync() alone does not render. Queued select/new + page actions
            // must never reach the formerly selected native browser/frame.
            self.reconcile_browser_selection(&pane, window, cx);
            let tab = self.model.tab_of(&pane).ok_or("no such browser pane")?.id.clone();
            let node = self.model.tab_mut(&tab).and_then(|t| t.root.find_mut(&pane)).ok_or("no such browser pane")?;
            let Kind::Block { content: Content::Browser { url, tabs } } = &mut node.kind else { return Err("target is not a browser pane".into()) };
            if name == "browser.navigate" {
                let input = Self::arg(args, "url").ok_or("browser.navigate url must be a string")?;
                *url = settings::browser::target(input, &self.person.browser_search);
                tabs.ensure(url);
                if let Some(item) = tabs.items.iter_mut().find(|t|t.id == tabs.selected) { item.url = url.clone(); }
                if let Some(slot) = self.browsers.get_mut(&pane) {
                    slot.url = url.clone();
                    slot.view.update(cx, |view, cx| view.navigate(url.clone(), window, cx));
                }
                return Ok(json!({"pane":pane}));
            }
            let slot = self.browsers.get(&pane).ok_or("browser has not been shown yet; show its tab first")?;
            slot.view.update(cx, |view, cx| match name {
                "browser.back" => view.back(window, cx),
                "browser.forward" => view.forward(window, cx),
                "browser.reload" => view.reload(window, cx),
                "browser.stop" => view.stop(window, cx),
                "browser.focus_address" => view.focus_address(window, cx),
                _ => unreachable!(),
            });
            Ok(json!({"pane":pane}))
        }
    }
}

/// A browser/address editor keeps ordinary editing keys, but must not hide the
/// person's explicit global/layout shortcuts behind its own editing bindings.
#[cfg(all(feature = "browser", target_os = "linux"))]
#[derive(Default)]
pub(super) struct BrowserKeys {
    bindings: Vec<(String, KeyBinding, String)>,
    pending: Vec<Keystroke>,
    since: Option<Instant>,
}

#[cfg(all(feature = "browser", target_os = "linux"))]
impl BrowserKeys {
    pub fn reload(&mut self) {
        self.pending.clear();
        self.bindings = crate::keys::listing().into_iter().filter_map(|(section, keys, action)| {
            if section != "global" && !section.starts_with("layout:") { return None; }
            let binding = KeyBinding::load(&keys, Box::new(ShellAction::new(action.clone())), None, false, None, &gpui::DummyKeyboardMapper).ok()?;
            Some((section, binding, action))
        }).collect();
    }

    fn action(&mut self, key: &Keystroke, layout: &str) -> (bool, Option<String>) {
        if self.since.is_some_and(|since| since.elapsed() > Duration::from_secs(1)) { self.pending.clear(); }
        self.pending.push(key.clone());
        let matches = |pending: &[Keystroke]| self.bindings.iter().rev().find_map(|(section, binding, action)| {
            if section != "global" && section.strip_prefix("layout:") != Some(layout) { return None; }
            binding.match_keystrokes(pending).map(|more| (more, action.clone()))
        });
        let mut matched = matches(&self.pending);
        if matched.is_none() && self.pending.len() > 1 {
            matched = matches(std::slice::from_ref(key));
            self.pending = vec![key.clone()];
        }
        self.since = Some(Instant::now());
        match matched {
            Some((true, _)) => (true, None),
            Some((false, action)) => {
                self.pending.clear();
                // These mean terminal passthrough, not application shortcuts.
                if action == "terminal" || action.starts_with("send:") { (false, None) }
                else { (true, Some(action)) }
            }
            None => { self.pending.clear(); (false, None) }
        }
    }
}

#[cfg(all(feature = "browser", target_os = "linux"))]
impl AppWindow {
    pub(super) fn install_browser_keys(&mut self, cx: &mut Context<Self>) -> Subscription {
        self.browser_keys.reload();
        let this = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            let _ = this.update(cx, |this, cx| {
                if !this.browsers.values().any(|slot| slot.view.read(cx).focus_handle(cx).contains_focused(window, cx)) { return; }
                let (matched, action) = this.browser_keys.action(&event.keystroke, &this.layout_name);
                if !matched { return; }
                cx.stop_propagation();
                if let Some(action) = action {
                    if let Ok(value) = serde_json::from_str::<serde_json::Map<String, Value>>(&action) {
                        match (crate::keys::CommandBinding { value }).command() {
                            Ok((name, args)) => this.run_key(&name, args, window, cx),
                            Err(error) => this.command_error("Key binding", &error, window, cx),
                        }
                    } else if action != "terminal" {
                        this.run_action(&action, window, cx);
                    }
                }
            });
        })
    }
}

#[cfg(all(feature = "browser", target_os = "linux"))]
impl BrowserSlot {
    fn belongs_to(&self, model: &Model, pane: &str) -> bool {
        model.space_of(pane).is_some_and(|space| space.id == self.workspace && space.machine == self.machine)
            && model.tab_of(pane).and_then(|tab| tab.root.find(pane)).is_some_and(|n| matches!(n.content(), Some(Content::Browser { .. })))
    }
}
