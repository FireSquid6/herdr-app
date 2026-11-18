//! Native catalog metadata projected into inert, URL-only saved layout data.
use super::*;

impl Panel {
    pub fn attached(&self) -> bool {
        matches!(self.shared.lock().unwrap().phase, Phase::Starting | Phase::Running)
    }

    pub fn saved_group(&self) -> Option<super::super::saved::SavedGroup> {
        let group = self.group.as_ref()?;
        let state = self.shared.lock().unwrap();
        if state.group != group.id || state.metadata_revision == 0 { return None; }
        let props = json!({"browser":{"version":1,"tabs":state.confirmed_metadata["tabs"],
            "selected":state.confirmed_metadata["selected"].as_str().unwrap_or_default()}});
        super::super::saved::SavedGroup::from_props(&props, &group.id, "about:blank").ok()
    }

    pub fn saved_props(&self, existing: &Value) -> Option<Value> {
        self.saved_group()?.to_props(existing).ok()
    }

    pub(super) fn group_command(&mut self, name: &str, args: &Value, cx: &mut Context<Self>) -> std::result::Result<Value, String> {
        let (selected, tabs) = {
            let state = self.shared.lock().unwrap();
            if !state.ready { return Err("Browser controller is not attached".into()); }
            (state.selected_tab.clone(), state.tabs.clone())
        };
        let tab = match args.get("tab") {
            None | Some(Value::Null) => selected.clone(),
            Some(Value::String(tab)) => tab.clone(),
            _ => return Err("Browser tab must be a string".into()),
        };
        let mut selection = None;
        let mut created = None;
        let (method, params): (&'static str, Value) = match name {
            "browser.stop" => ("browser.stop", json!({"confirmed":true})),
            "browser.tab.new" => {
                let id = args["id"].as_str().ok_or("New tab requires an allocated stable ID")?;
                let url = args["url"].as_str().ok_or("Browser URL must be a string")?;
                if tabs.iter().any(|tab| tab["id"] == id) { return Err("Browser tab ID already exists".into()); }
                created = Some(json!({"id":id,"url":url,"target":null}));
                selection = Some(id.to_owned());
                ("browser.tab.create", json!({"id":id,"url":url}))
            }
            "browser.tab.next" | "browser.tab.previous" => {
                let index = tabs.iter().position(|tab| tab["id"] == selected).ok_or("No selected browser tab")?;
                let next = if name == "browser.tab.next" { (index + 1) % tabs.len() } else { (index + tabs.len() - 1) % tabs.len() };
                let id = tabs[next]["id"].as_str().ok_or("Invalid browser tab identity")?.to_owned();
                selection = Some(id.clone());
                ("browser.tab.select", json!({"tab":id}))
            }
            "browser.tab.select" | "browser.tab.close" => {
                if !tabs.iter().any(|entry| entry["id"] == tab) { return Err("Browser tab does not belong to this pane".into()); }
                if name == "browser.tab.select" { selection = Some(tab.clone()); }
                (if name == "browser.tab.select" { "browser.tab.select" } else { "browser.tab.close" }, json!({"tab":tab}))
            }
            "browser.navigate" => ("browser.navigate", json!({"tab":selected,"url":args["url"].as_str().ok_or("Browser URL must be a string")?})),
            "browser.back" | "browser.forward" => ("browser.history", json!({"tab":selected,"direction":if name == "browser.back" {"back"} else {"forward"}})),
            "browser.reload" => ("browser.reload", json!({"tab":selected})),
            "browser.devtools.open" => ("browser.devtools.open", json!({"tab":selected})),
            "browser.devtools.close" => ("browser.devtools.close", json!({"tab":selected})),
            _ => return Err("Unsupported browser command".into()),
        };
        self.invalidate_clipboard();
        self.commands.try_send(Command { method, params, follow:false, clipboard:None, context:None })
            .map_err(|error| format!("Browser command queue full/disconnected: {error}"))?;
        if let Some(selection) = selection {
            let mut state = self.shared.lock().unwrap();
            if let Some(created) = created { state.tabs.push(created); }
            state.selected = state.tabs.iter().find(|tab| tab["id"] == selection)
                .map(Snapshot::entry_target).unwrap_or_default().to_owned();
            state.selected_tab = selection;
            state.source_epoch += 1;
            state.page = None;
            state.devtools = None;
            state.revision += 1;
        }
        cx.notify();
        Ok(json!({"queued":true}))
    }
}
