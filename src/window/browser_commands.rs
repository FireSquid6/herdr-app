//! Trusted app command adapter for the core-owned browser. Never routes a
//! remote workspace through the desktop's local browser connection.
use super::*;

impl AppWindow {
    pub(super) fn browser_chrome_instance(&self, node: &Node) -> (String, Instance) {
        let chrome = match node.content() {
            Some(Content::Mod { props, .. }) => props.get("chrome").and_then(Value::as_str)
                .filter(|name| !name.is_empty()).unwrap_or(&self.person.browser_chrome),
            _ => &self.person.browser_chrome,
        };
        (format!("{}#browser", node.id), Instance { name: chrome.into(), props: json!({"pane":node.id}) })
    }

    pub(super) fn prepare_browser_chrome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(runtime) = self.runtime.clone() else { return; };
        let owners = self.model.spaces.iter().flat_map(|space| space.tabs.iter().flat_map(move |tab|
            tab.root.walk().into_iter().filter_map(move |node| match node.content() {
                Some(Content::Mod { name, props }) if name == crate::browser::NATIVE_SLOT =>
                    Some((node.id.clone(), space.id.clone(), space.machine.clone().unwrap_or_default(), props.clone())),
                _ => None,
            }))).collect::<Vec<_>>();
        self.browsers.retain(|pane, _| owners.iter().any(|owner| &owner.0 == pane));
        for (pane, workspace, machine, props) in owners {
            if !self.browsers.contains_key(&pane) {
                let Ok(saved) = crate::browser::saved::SavedGroup::from_props(&props, &pane, &self.person.browser_url) else { continue; };
                let viewer = self.browser_viewers.entry(format!("{machine}:{workspace}")).or_default().clone();
                let config = crate::browser::panel::Config { workspace, machine, viewer,
                    group:Some(crate::browser::panel::GroupConfig { id:pane.clone(), saved }), attached:false, visible:false };
                self.browsers.insert(pane.clone(), cx.new(|cx| crate::browser::panel::Panel::configured(config, window, cx)));
            }
            let Some(panel) = self.browsers.get(&pane).cloned() else { continue; };
            let copy = self.copies.get(&format!("{pane}#browser"));
            let root = copy.and_then(|copy| copy.root.clone());
            let id = copy.map_or(0, |copy| copy.id);
            panel.update(cx, |panel, cx| panel.set_chrome(pane, root, id, runtime.clone(), cx));
        }
    }

    pub(super) fn sync_browser_saved(&mut self, cx: &App) {
        if self.require_document(None).is_err() { return; }
        let updates = self.browsers.iter().filter_map(|(pane, panel)| {
            let tab = self.model.tab_of(pane)?;
            if self.model.space_of(pane)?.machine.is_some() { return None; }
            let Some(Content::Mod { name, props }) = tab.root.find(pane)?.content() else { return None; };
            if name != crate::browser::NATIVE_SLOT { return None; }
            let saved = panel.read(cx).saved_props(props)?;
            (saved != *props).then(|| (tab.id.clone(), pane.clone(), saved))
        }).collect::<Vec<_>>();
        for (tab, pane, saved) in updates {
            if let Some(node) = self.model.tab_mut(&tab).and_then(|tab| tab.root.find_mut(&pane)) {
                if let Kind::Block { content:Content::Mod { props, .. } } = &mut node.kind { *props = saved; }
            }
        }
    }

    pub(super) fn browser_command(
        &mut self, name: &str, args: &Value, window: &mut Window, cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        if name == "browser.new" {
            let workspace = Self::owned(args, "space")
                .or_else(|| self.model.active_space().map(|space| space.id.clone()))
                .ok_or("No workspace selected")?;
            let space = self.model.space(&workspace).ok_or("No such workspace")?;
            if space.machine.is_some() {
                return Err("Remote browser route unavailable: local fallback is forbidden".into());
            }
            self.require_document(None)?;
            self.model.active = Some(workspace.clone());
            self.open_browser(window, cx);
            let pane = self.model.active_tab().and_then(|tab| tab.root.walk().into_iter().find(|node|
                matches!(node.content(), Some(Content::Mod { name, .. }) if name == crate::browser::NATIVE_SLOT)))
                .map(|node| node.id.clone()).ok_or("Browser pane was not created")?;
            return Ok(json!({"pane":pane,"workspace":workspace}));
        }
        let pane = Self::owned(args, "pane").or_else(|| self.model.focused_pane())
            .ok_or("No browser pane focused")?;
        let node = self.model.tab_of(&pane).and_then(|tab| tab.root.find(&pane)).ok_or("No such pane")?;
        if !matches!(node.content(), Some(Content::Mod { name, .. }) if name == crate::browser::NATIVE_SLOT) {
            return Err("The target is not a browser pane".into());
        }
        let space = self.model.space_of(&pane).ok_or("Browser workspace disappeared")?;
        if space.machine.is_some() {
            return Err("Remote browser route unavailable: local fallback is forbidden".into());
        }
        let panel = self.browsers.get(&pane).cloned().ok_or("Browser pane metadata is invalid or viewer is unavailable")?;
        let mut params = args.clone();
        if name == "browser.navigate" || name == "browser.tab.new" {
            let input = match params.get("url") {
                None | Some(Value::Null) if name == "browser.tab.new" => &self.person.browser_url,
                Some(Value::String(url)) => url,
                _ => return Err("Browser URL must be a string".into()),
            };
            params["url"] = json!(if input == "about:blank" { input.clone() }
                else { crate::settings::browser::target(input, &self.person.browser_search) });
        }
        if name == "browser.tab.new" { params["id"] = json!(self.model.fresh_id("bt-")); }
        if name == "browser.focus_address" {
            self.model.focus_pane(&pane);
            self.focused_pane = Some(pane.clone());
            self.publish(cx);
        }
        panel.update(cx, |panel, cx| panel.dispatch_command(name, &params, window, cx))
    }
}
