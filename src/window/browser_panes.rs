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
        let _ = (window, cx);
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
                        let Content::Browser { url } = node.content()? else { return None };
                        Some((node.id.clone(), space.id.clone(), space.machine.clone(), url.clone()))
                    }))
                }).collect()
            } else { Vec::new() };
            for (pane, slot) in &self.browsers {
                let shown = visible.iter().any(|(id, _, _, _)| id == pane);
                slot.view.update(cx, |view, cx| view.set_visible(shown, window, cx));
            }
            for (pane, workspace, machine, url) in visible {
                if let Some(slot) = self.browsers.get_mut(&pane) {
                    if slot.url != url {
                        slot.url = url.clone();
                        slot.view.update(cx, |view, cx| view.navigate(url, window, cx));
                    }
                    continue;
                }
                let options = BrowserOptions { workspace: workspace.clone(), pane: pane.clone(), url: url.clone(), profile_root: mods::app_dir().join("browser-profiles"), remote: machine.is_some() };
                let view = cx.new(|cx| BrowserPane::new(options, window, cx));
                let target = pane.clone();
                let owner = workspace.clone();
                let on = machine.clone();
                let events = cx.subscribe_in(&view, window, move |this, source, event: &BrowserEvent, window, cx| {
                    // A queued event from an old owner/session must never update its replacement.
                    if !this.browsers.get(&target).is_some_and(|s| s.workspace == owner && s.machine == on && s.view == *source) { return; }
                    let BrowserEvent::Navigated(url) = event;
                    if let Some(tab) = this.model.tab_of(&target).map(|t| t.id.clone())
                        && let Some(node) = this.model.tab_mut(&tab).and_then(|t| t.root.find_mut(&target))
                        && let Kind::Block { content: Content::Browser { url: saved } } = &mut node.kind
                    {
                        *saved = url.clone();
                        if let Some(slot) = this.browsers.get_mut(&target) { slot.url = url.clone(); }
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

    pub(super) fn render_browser(&self, pane: &str, cx: &mut Context<Self>) -> AnyElement {
        let pane = pane.to_owned();
        #[cfg(all(feature = "browser", target_os = "linux"))]
        if let Some(slot) = self.browsers.get(&pane) {
            return div().size_full().capture_any_mouse_down(cx.listener(move |this, _, _, cx| {
                this.model.focus_pane(&pane);
                this.focused_pane = Some(pane.clone());
                this.synced_model_pane = this.model.focused_pane();
                this.focused_block = None;
                this.publish(cx);
                this.save();
                cx.notify();
            })).child(slot.view.clone()).into_any_element();
        }
        let _ = cx;
        div().size_full().flex().items_center().justify_center().text_color(c("dim"))
            .child("Browser unavailable: requires the optional Linux browser build and runtime.").into_any_element()
    }

    pub(super) fn browser_command(&mut self, name: &str, args: &Value, window: &mut Window, cx: &mut Context<Self>) -> Result<Value, String> {
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
                let node = Node::block(pane.clone(), Content::Browser { url });
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
            let tab = self.model.tab_of(&pane).ok_or("no such browser pane")?.id.clone();
            let node = self.model.tab_mut(&tab).and_then(|t| t.root.find_mut(&pane)).ok_or("no such browser pane")?;
            let Kind::Block { content: Content::Browser { url } } = &mut node.kind else { return Err("target is not a browser pane".into()) };
            if name == "browser.navigate" {
                let input = Self::arg(args, "url").ok_or("browser.navigate url must be a string")?;
                *url = settings::browser::target(input, &self.person.browser_search);
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
