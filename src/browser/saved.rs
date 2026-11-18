//! URL/order/selection metadata only. No targets, engine state or website bridge.
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub const MAX_TABS: usize = 64;
pub const MAX_URL_BYTES: usize = 16_384;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SavedTab {
    pub id: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    DroppedEntry { index: usize },
    DroppedId { index: usize },
    DuplicateId { index: usize },
    ScopedId { index: usize },
    ReplacedUrl { index: usize },
    InvalidSelection,
    InvalidLegacyUrl,
    InvalidFallback,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SavedError {
    MissingPaneId,
    InvalidProps,
    InvalidBrowser,
    UnsupportedVersion,
    InvalidLegacyTabs,
    TooManyTabs { count: usize, limit: usize },
}
impl std::fmt::Display for SavedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingPaneId => f.write_str("Saved browser group requires a pane ID"),
            Self::InvalidProps => f.write_str("Saved browser props must be an object or null"),
            Self::InvalidBrowser => f.write_str("Malformed saved browser group"),
            Self::UnsupportedVersion => f.write_str("Unsupported saved browser metadata version"),
            Self::InvalidLegacyTabs => f.write_str("Malformed legacy browser tabs"),
            Self::TooManyTabs { count, limit } => write!(f,"Saved browser group has {count} entries; limit is {limit}; no entries were truncated"),
        }
    }
}
impl std::error::Error for SavedError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedGroup {
    tabs: Vec<SavedTab>,
    selected: String,
    problems: Vec<Problem>,
}

#[cfg(test)]
mod tests {
    use super::{MAX_TABS, Problem, SavedError, SavedGroup};
    use serde_json::json;

    #[test]
    fn empty_version_one_is_not_a_legacy_url_and_unknown_versions_fail_closed() {
        let group = SavedGroup::from_props(&json!({"url":"https://stale.test/","browser":{"version":1,"tabs":[],"selected":""}}),"pane","about:blank").unwrap();
        assert!(group.tabs().is_empty());
        assert_eq!(group.selected(),"");
        assert_eq!(group.restore_params(),json!({"tabs":[],"selected":null}));
        for browser in [json!({"version":2,"tabs":[]}),json!({"version":"1","tabs":[]})] {
            assert_eq!(SavedGroup::from_props(&json!({"url":"https://stale.test/","browser":browser}),"pane","about:blank"),Err(SavedError::UnsupportedVersion));
        }
        assert_eq!(SavedGroup::from_props(&json!({"browser":null}),"pane","about:blank"),Err(SavedError::InvalidBrowser));
        assert_eq!(SavedGroup::from_props(&json!({"browser":{"version":1}}),"pane","about:blank"),Err(SavedError::InvalidBrowser));
    }

    #[test]
    fn malformed_duplicates_and_unsafe_urls_are_reported_without_executing_or_searching() {
        let group = SavedGroup::from_props(&json!({"browser":{"version":1,"selected":"missing","tabs":[
            {"id":"p:first","url":"javascript:alert(1)","target":"NEVER-PERSIST","history":["secret"]},
            {"id":"p:first","url":"https://duplicate.test/"},{"id":"unscoped","url":"https://bad-id.test/"},
            {"id":"","url":"https://empty-id.test/"},false,{"id":"p:last","url":"https://valid.test/π?q=one"}]}}),"p","https://fallback.test/").unwrap();
        assert_eq!(group.tabs().len(),2);
        assert_eq!(group.tabs()[0].url,"https://fallback.test/");
        assert_eq!(group.tabs()[1].url,"https://valid.test/π?q=one");
        assert_eq!(group.selected(),"p:first");
        assert!(group.problems().contains(&Problem::ReplacedUrl { index:0 }));
        assert!(group.problems().contains(&Problem::DuplicateId { index:1 }));
        assert!(group.problems().contains(&Problem::DroppedId { index:2 }));
        assert!(group.problems().contains(&Problem::DroppedId { index:3 }));
        assert!(group.problems().contains(&Problem::DroppedEntry { index:4 }));
        assert!(group.problems().contains(&Problem::InvalidSelection));
        let saved = group.to_props(&json!({})).unwrap().to_string();
        assert!(!saved.contains("NEVER-PERSIST"));
        assert!(!saved.contains("history"));
        assert!(!saved.contains("javascript:"));
        let legacy = SavedGroup::from_props(&json!({"url":"file:///private","tabs":{"items":[]}}),"p","javascript:bad").unwrap();
        assert_eq!(legacy.tabs()[0].url,"about:blank");
        assert!(legacy.problems().contains(&Problem::InvalidLegacyUrl));
        assert!(legacy.problems().contains(&Problem::InvalidFallback));
    }

    #[test]
    fn stable_global_and_scoped_ids_survive_moves_while_legacy_remote_ids_hash_deterministically() {
        let id = "bt-0123456789abcdef0123456789abcdef";
        let props = json!({"browser":{"version":1,"tabs":[{"id":id,"url":"https://one.test/"},
            {"id":"source-pane:initial","url":"https://two.test/"}],"selected":"source-pane:initial"}});
        let moved = SavedGroup::from_props(&props,"destination-pane","about:blank").unwrap();
        assert_eq!(moved.tabs()[0].id,id);
        assert_eq!(moved.tabs()[1].id,"source-pane:initial");
        assert!(moved.problems().is_empty());
        let legacy = SavedGroup::from_props(&json!({"url":"https://saved.test/"}),"remote/pane","about:blank").unwrap();
        assert_eq!(legacy.selected(),"bt-53e024b05d68b941e8a16abca600a5cf53f56ba0c07ab8111ffc3afaed4e370d");
        let again = SavedGroup::from_props(&legacy.to_props(&json!({})).unwrap(),"other-pane","about:blank").unwrap();
        assert_eq!(again.tabs(),legacy.tabs());
    }

    #[test]
    fn legacy_duplicate_ids_keep_the_first_url_and_invalid_selection_chooses_first() {
        let group = SavedGroup::from_props(&json!({"url":"https://old-selected.test/","tabs":{"items":[
            {"id":"","url":"https://empty-id.test/"},{"id":"a","url":"https://first.test/"},
            {"id":"a","url":"https://duplicate.test/"},{"id":"bad/id","url":"https://malformed.test/"}],"selected":"missing"}}),"p","about:blank").unwrap();
        assert_eq!(group.tabs().len(),1);
        assert_eq!(group.tabs()[0].url,"https://first.test/");
        assert_eq!(group.selected(),"p:a");
        assert!(group.problems().contains(&Problem::DroppedId { index:0 }));
        assert!(group.problems().contains(&Problem::DuplicateId { index:2 }));
        assert!(group.problems().contains(&Problem::DroppedId { index:3 }));
        assert!(group.problems().contains(&Problem::InvalidSelection));
        let no_items = SavedGroup::from_props(&json!({"url":"https://old-selected.test/","tabs":{"items":[],"selected":"missing"}}),"p","https://fallback.test/").unwrap();
        assert_eq!(no_items.tabs()[0].url,"https://old-selected.test/");
        assert_eq!(no_items.selected(),"p:initial");
    }

    #[test]
    fn limits_are_explicit_errors_not_silent_truncation_and_props_are_not_coerced() {
        let items = (0..MAX_TABS).map(|i|json!({"id":format!("p:{i}"),"url":"about:blank"})).collect::<Vec<_>>();
        let mut props = json!({"browser":{"version":1,"tabs":items,"selected":"p:63"}});
        assert_eq!(SavedGroup::from_props(&props,"p","about:blank").unwrap().tabs().len(),64);
        props["browser"]["tabs"].as_array_mut().unwrap().push(json!({"id":"p:64","url":"about:blank"}));
        assert_eq!(SavedGroup::from_props(&props,"p","about:blank"),Err(SavedError::TooManyTabs { count:65,limit:64 }));
        let legacy = json!({"url":"https://saved.test/","tabs":{"items":props["browser"]["tabs"]}});
        assert_eq!(SavedGroup::from_props(&legacy,"p","about:blank"),Err(SavedError::TooManyTabs { count:65,limit:64 }));
        assert_eq!(SavedGroup::from_props(&json!([]),"p","about:blank"),Err(SavedError::InvalidProps));
        assert_eq!(SavedGroup::from_props(&json!(null),"","about:blank"),Err(SavedError::MissingPaneId));
        let group = SavedGroup::from_props(&json!(null),"p","about:blank").unwrap();
        assert_eq!(group.to_props(&json!("must not discard")),Err(SavedError::InvalidProps));
        assert_eq!(group.to_props(&json!({"browser":{"version":2,"future":"must not discard"}})),Err(SavedError::UnsupportedVersion));
    }

    #[test]
    fn saved_urls_are_web_only_and_never_address_searches() {
        for url in ["about:blank","HTTPS://example.test/path?q=one#two","http://localhost:3000/","http://[::1]:8000/","https://例.test/π"] {
            assert!(super::safe_url(url),"{url}");
        }
        for url in ["javascript:alert(1)","file:///tmp/page","data:text/html,test","cats and dogs","example.test",
            "https:///missing","http://host:bad/","http://host:0/","http://host:65536/","http://user:secret@host/",
            "http://host\\evil/","https://host/\nheader","about:config"] {
            assert!(!super::safe_url(url),"{url}");
        }
    }

    #[test]
    fn legacy_ids_are_scoped_once_and_selected_url_order_survives_save_load() {
        let props = json!({"chrome":"owned","custom":{"keep":true},"url":"https://selected.test/",
            "tabs":{"items":[{"id":"initial","url":"https://first.test/"},{"id":"two","url":"https://selected.test/"}],"selected":"two"}});
        let group = SavedGroup::from_props(&props,"pane-a","about:blank").unwrap();
        assert_eq!(group.tabs()[0].id,"pane-a:initial");
        assert_eq!(group.selected(),"pane-a:two");
        let other = SavedGroup::from_props(&props,"pane-b","about:blank").unwrap();
        assert_ne!(group.tabs()[0].id,other.tabs()[0].id);
        let saved = group.to_props(&props).unwrap();
        assert_eq!(saved["chrome"],"owned");
        assert_eq!(saved["custom"],json!({"keep":true}));
        assert!(saved.get("url").is_none());
        assert!(saved.get("tabs").is_none());
        assert_eq!(saved["browser"]["version"],1);
        let loaded = SavedGroup::from_props(&saved,"pane-a","about:blank").unwrap();
        assert_eq!(loaded.tabs(),group.tabs());
        assert_eq!(loaded.selected(),group.selected());
        assert!(loaded.problems().is_empty());
        assert_eq!(loaded.restore_params(),json!({"tabs":[{"id":"pane-a:initial","url":"https://first.test/"},
            {"id":"pane-a:two","url":"https://selected.test/"}],"selected":"pane-a:two"}));
    }
}

impl SavedGroup {
    /// Version 1 is authoritative, including an intentionally empty tab vector.
    /// Legacy URL-only props are normalized only after their pane ID is known.
    pub fn from_props(props: &Value, pane_id: &str, fallback_url: &str) -> Result<Self, SavedError> {
        if pane_id.is_empty() { return Err(SavedError::MissingPaneId); }
        if !props.is_null() && !props.is_object() { return Err(SavedError::InvalidProps); }
        let mut problems = Vec::new();
        let fallback = if safe_url(fallback_url) { fallback_url } else {
            problems.push(Problem::InvalidFallback);
            "about:blank"
        };
        let modern = props.get("browser");
        let (items, selection, versioned) = if let Some(browser) = modern {
            if !browser.is_object() { return Err(SavedError::InvalidBrowser); }
            if browser["version"].as_u64() != Some(1) { return Err(SavedError::UnsupportedVersion); }
            let items = browser["tabs"].as_array().ok_or(SavedError::InvalidBrowser)?;
            (items.as_slice(),browser["selected"].as_str().unwrap_or(""),true)
        } else {
            let legacy = props.get("tabs");
            let items = match legacy {
                None | Some(Value::Null) => &[][..],
                Some(Value::Object(tabs)) => match tabs.get("items") {
                    None => &[][..],
                    Some(Value::Array(items)) => items.as_slice(),
                    _ => return Err(SavedError::InvalidLegacyTabs),
                },
                _ => return Err(SavedError::InvalidLegacyTabs),
            };
            (items,props["tabs"]["selected"].as_str().unwrap_or(""),false)
        };
        if versioned && props["browser"]["selected"].as_str().is_none() {
            problems.push(Problem::InvalidSelection);
        }
        if items.len() > MAX_TABS {
            return Err(SavedError::TooManyTabs { count:items.len(),limit:MAX_TABS });
        }
        let mut tabs = Vec::new();
        let mut seen = HashSet::new();
        let mut selected = None;
        for (index, item) in items.iter().enumerate() {
            let (Some(raw_id),Some(raw_url)) = (item["id"].as_str(),item["url"].as_str()) else {
                problems.push(Problem::DroppedEntry { index });
                continue;
            };
            if !id_chars(raw_id) || versioned && (!valid_id(raw_id) || !stable_namespace(raw_id)) {
                problems.push(Problem::DroppedId { index });
                continue;
            }
            let id = if versioned { raw_id.to_owned() } else { legacy_id(pane_id,raw_id) };
            if !seen.insert(id.clone()) {
                problems.push(Problem::DuplicateId { index });
                continue;
            }
            if id != raw_id { problems.push(Problem::ScopedId { index }); }
            let url = if safe_url(raw_url) { raw_url } else {
                problems.push(Problem::ReplacedUrl { index });
                fallback
            };
            if raw_id == selection { selected = Some(id.clone()); }
            tabs.push(SavedTab { id,url:url.into() });
        }
        if tabs.is_empty() && !versioned {
            let url = match props.get("url") {
                Some(Value::String(url)) if safe_url(url) => url.as_str(),
                Some(_) => { problems.push(Problem::InvalidLegacyUrl); fallback },
                None => fallback,
            };
            let id = legacy_id(pane_id,"initial");
            selected = Some(id.clone());
            tabs.push(SavedTab { id,url:url.into() });
        }
        let selected = selected.unwrap_or_else(|| {
            if (!selection.is_empty() || !tabs.is_empty()) && !problems.contains(&Problem::InvalidSelection) {
                problems.push(Problem::InvalidSelection);
            }
            tabs.first().map(|tab|tab.id.clone()).unwrap_or_default()
        });
        Ok(Self { tabs,selected,problems })
    }

    pub fn tabs(&self) -> &[SavedTab] { &self.tabs }
    pub fn selected(&self) -> &str { &self.selected }
    pub fn problems(&self) -> &[Problem] { &self.problems }

    /// Preserve custom native-slot props, replacing only consumed browser data.
    pub fn to_props(&self, existing: &Value) -> Result<Value, SavedError> {
        if let Some(browser) = existing.get("browser") {
            if !browser.is_object() { return Err(SavedError::InvalidBrowser); }
            if browser["version"].as_u64() != Some(1) { return Err(SavedError::UnsupportedVersion); }
        }
        let mut props = match existing {
            Value::Null => serde_json::Map::new(),
            Value::Object(props) => props.clone(),
            _ => return Err(SavedError::InvalidProps),
        };
        props.remove("url");
        props.remove("tabs");
        props.insert("browser".into(),json!({"version":1,"tabs":self.tabs,"selected":self.selected}));
        Ok(Value::Object(props))
    }

    /// Restore input, not an engine command. Warm core membership wins over it;
    /// the caller controls which visible group's selected entry is instantiated.
    pub fn restore_params(&self) -> Value {
        // The wire catalog uses null for no selected tab. Saved v1 props keep
        // their string representation; an empty group must not mint a page.
        let selected = (!self.selected.is_empty()).then_some(self.selected.as_str());
        json!({"tabs":self.tabs,"selected":selected})
    }
}

fn id_chars(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b|b.is_ascii_alphanumeric() || b":_-".contains(&b))
}
fn valid_id(id: &str) -> bool { id.len() <= 128 && id_chars(id) }
fn global_id(id: &str) -> bool {
    id.strip_prefix("bt-").is_some_and(|value|
        matches!(value.len(),32|64) && value.bytes().all(|b|b.is_ascii_hexdigit())
        || value.len() == 36 && value.bytes().enumerate().all(|(i,b)|
            if matches!(i,8|13|18|23) { b == b'-' } else { b.is_ascii_hexdigit() }))
}
fn stable_namespace(id: &str) -> bool {
    // Scoped IDs remain stable after a live move to a different pane.
    global_id(id) || id.split_once(':').is_some_and(|(pane,tab)|!pane.is_empty() && !tab.is_empty())
}
fn legacy_id(pane: &str, id: &str) -> String {
    if global_id(id) || valid_id(id) && id.starts_with(&format!("{pane}:")) { return id.into(); }
    let scoped = format!("{pane}:{id}");
    if valid_id(&scoped) && !pane.contains(':') && !id.contains(':') { return scoped; }
    // Length-framed SHA-256 avoids delimiter collisions and unstable hash seeds.
    let mut hash = Sha256::new();
    hash.update(b"herdr-saved-browser-tab-v1\0");
    hash.update((pane.len() as u64).to_le_bytes());
    hash.update(pane.as_bytes());
    hash.update(id.as_bytes());
    let digest: String = hash.finalize().iter().map(|byte|format!("{byte:02x}")).collect();
    format!("bt-{digest}")
}

/// Persisted URLs are never interpreted as searches or executable local schemes.
/// Keep accepted strings exact; this validator performs no navigation or I/O.
pub fn safe_url(url: &str) -> bool {
    if url == "about:blank" { return true; }
    if url.len() > MAX_URL_BYTES || url.chars().any(|c|c.is_whitespace() || c.is_control() || c == '\\') { return false; }
    let Some((scheme,rest)) = url.split_once("://") else { return false };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") { return false; }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') { return false; }
    let (host,port) = if let Some(ipv6) = authority.strip_prefix('[') {
        let Some((host,tail)) = ipv6.split_once(']') else { return false };
        if host.parse::<std::net::Ipv6Addr>().is_err() { return false; }
        (host,if tail.is_empty() { None } else {
            let Some(port) = tail.strip_prefix(':') else { return false };
            Some(port)
        })
    } else {
        let (host,port) = authority.split_once(':').map_or((authority,None),|(host,port)|(host,Some(port)));
        if !host.chars().all(|c|c.is_alphanumeric() || matches!(c,'.'|'-'|'_'))
            || host.trim_matches('.').is_empty() { return false; }
        (host,port)
    };
    !host.is_empty() && port.is_none_or(|port| !port.is_empty() && port.bytes().all(|b|b.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port|port != 0))
}
