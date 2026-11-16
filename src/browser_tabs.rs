//! Persisted browser-tab identity, independent of native pages and workspace tabs.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BrowserTabs {
    #[serde(default)]
    pub items: Vec<BrowserTab>,
    #[serde(default)]
    pub selected: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrowserTab {
    pub id: String,
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_restored_ids_cannot_alias_live_pages() {
        let mut tabs = BrowserTabs { items: vec![
            BrowserTab { id: "".into(), url: "https://empty.test".into() },
            BrowserTab { id: "a".into(), url: "https://first.test".into() },
            BrowserTab { id: "a".into(), url: "https://duplicate.test".into() },
        ], selected: "missing".into() };
        tabs.ensure("https://fallback.test");
        assert_eq!(tabs.items.len(), 1);
        assert_eq!(tabs.selected, "a");
        assert_eq!(tabs.url(), Some("https://first.test"));
    }
    #[test]
    fn legacy_migration_and_close_preserve_selection_and_order() {
        let mut tabs = BrowserTabs::default();
        tabs.ensure("https://old.test");
        let first = tabs.selected.clone();
        tabs.insert("second".into(), "https://second.test".into());
        tabs.insert("third".into(), "https://third.test".into());
        assert!(tabs.move_to("third", 0));
        assert_eq!(tabs.items[0].id, "third");
        assert!(tabs.close("third"));
        assert_eq!(tabs.selected, first);
        let restored: BrowserTabs = serde_json::from_str(&serde_json::to_string(&tabs).unwrap()).unwrap();
        assert_eq!(restored, tabs);
        assert!(!tabs.select("missing"));
        assert!(!tabs.close("missing"));
    }
}

impl BrowserTabs {
    pub fn is_empty(&self) -> bool { self.items.is_empty() }
    /// Old URL-only documents become one stable tab on first ownership.
    pub fn ensure(&mut self, url: &str) {
        let mut seen = std::collections::HashSet::new();
        self.items.retain(|tab| !tab.id.is_empty() && seen.insert(tab.id.clone()));
        if self.items.is_empty() {
            self.insert("initial".into(), url.into());
        }
        if !self.items.iter().any(|t| t.id == self.selected) {
            self.selected = self.items[0].id.clone();
        }
    }
    pub fn insert(&mut self, id: String, url: String) {
        self.selected = id.clone();
        self.items.push(BrowserTab { id, url });
    }
    pub fn select(&mut self, id: &str) -> bool {
        if !self.items.iter().any(|t| t.id == id) { return false; }
        self.selected = id.into();
        true
    }
    pub fn move_to(&mut self, id: &str, index: usize) -> bool {
        let Some(from) = self.items.iter().position(|t| t.id == id) else { return false };
        let tab = self.items.remove(from);
        self.items.insert(index.min(self.items.len()), tab);
        true
    }
    pub fn close(&mut self, id: &str) -> bool {
        let Some(index) = self.items.iter().position(|t| t.id == id) else { return false };
        self.items.remove(index);
        if self.selected == id {
            self.selected = self.items.get(index.min(self.items.len().saturating_sub(1))).map(|t| t.id.clone()).unwrap_or_default();
        }
        true
    }
    pub fn url(&self) -> Option<&str> {
        self.items.iter().find(|t| t.id == self.selected).map(|t| t.url.as_str())
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::layout::Content;
    #[test]
    fn url_only_documents_roundtrip_without_losing_legacy_field() {
        let mut old: Content = serde_json::from_str(r#"{"url":"https://saved.test/form?q=one"}"#).unwrap();
        let Content::Browser { url, tabs } = &mut old else { panic!() };
        assert!(tabs.is_empty());
        tabs.ensure(url);
        let id = tabs.selected.clone();
        let saved = serde_json::to_value(&old).unwrap();
        assert_eq!(saved["url"], "https://saved.test/form?q=one");
        let Content::Browser { url, mut tabs } = serde_json::from_value(saved).unwrap() else { panic!() };
        tabs.ensure(&url);
        assert_eq!(tabs.selected, id);
        assert_eq!(tabs.items[0].url, url);
    }
    #[test]
    fn closing_nonselected_keeps_selection_and_last_is_empty() {
        let mut tabs = BrowserTabs::default();
        tabs.ensure("https://one.test");
        tabs.insert("two".into(), "https://two.test".into());
        assert!(tabs.close("initial"));
        assert_eq!(tabs.selected, "two");
        assert!(tabs.close("two"));
        assert!(tabs.is_empty());
        assert!(tabs.url().is_none());
    }
}
