//! Read old URL blocks without restoring a browser engine/runtime Content variant.
use super::{Content, PaneFrame};
use serde::Deserialize;
use serde_json::{Value, json};

#[cfg(test)]
mod tests {
    use crate::{browser::{NATIVE_SLOT, saved::SavedGroup}, layout::{Content, Node}};
    use serde_json::json;

    #[test]
    fn current_content_roundtrips_and_takes_precedence_over_extra_url_fields() {
        for value in [json!({"terminal":"t1"}),json!({"mod":"custom","props":{"url":"file:///custom-prop","chrome":"skin"}}),
            json!({"tab":"active"}),json!({"tab":"saved-tab","pane":{"shadow":"sm"}}),json!({"agent":"focused"})] {
            let content: Content = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(&content).unwrap(),value);
            let mut extra = value.clone();
            extra["url"] = json!("https://must-not-steal.test/");
            assert_eq!(serde_json::from_value::<Content>(extra).unwrap(),content);
        }
        let old: Content = serde_json::from_value(json!({"context":"active"})).unwrap();
        assert_eq!(serde_json::to_value(old).unwrap(),json!({"tab":"active"}));
    }

    #[test]
    fn legacy_two_tab_document_reaches_the_pure_model_without_dropping_other_panes() {
        let document: crate::mirror::Document = serde_json::from_value(json!({"spaces":[{"id":"space","tabs":[{
            "id":"outer","name":"Saved","root":{"id":"root","type":"row","children":[
                {"id":"legacy","type":"block","content":{"url":"https://two.test/","tabs":{"items":[
                    {"id":"initial","url":"https://one.test/"},{"id":"two","url":"https://two.test/"}],"selected":"two"}}},
                {"id":"terminal-pane","type":"block","content":{"terminal":"t1"}}
            ]}}]}]})).unwrap();
        let mut model = crate::model::Model::default();
        document.replace(&mut model,None);
        let root = &model.tab("outer").unwrap().root;
        assert_eq!(root.id,"root");
        assert!(matches!(root.find("terminal-pane").unwrap().content(),Some(Content::Terminal { terminal }) if terminal == "t1"));
        let Some(Content::Mod { name,props }) = root.find("legacy").unwrap().content() else { panic!("expected native slot") };
        assert_eq!(name,NATIVE_SLOT);
        let group = SavedGroup::from_props(props,"legacy","about:blank").unwrap();
        assert_eq!(group.tabs().iter().map(|tab|tab.url.as_str()).collect::<Vec<_>>(),vec!["https://one.test/","https://two.test/"]);
        assert_eq!(group.selected(),"legacy:two");
        let saved = serde_json::to_value(crate::mirror::Document::capture(&model,None)).unwrap();
        let loaded: crate::mirror::Document = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded,document);
    }

    #[test]
    fn old_url_block_reads_as_a_native_slot_without_changing_its_node_identity() {
        let node: Node = serde_json::from_value(json!({"id":"saved-pane","type":"block",
            "content":{"url":"https://saved.test/form?q=one"}})).unwrap();
        assert_eq!(node.id,"saved-pane");
        let Some(Content::Mod { name,props }) = node.content() else { panic!("expected inert native slot") };
        assert_eq!(name,NATIVE_SLOT);
        let group = SavedGroup::from_props(props,&node.id,"about:blank").unwrap();
        assert_eq!(group.tabs()[0].url,"https://saved.test/form?q=one");
        assert_eq!(group.selected(),"saved-pane:initial");
        let saved = serde_json::to_value(&node).unwrap();
        assert_eq!(saved["content"]["mod"],NATIVE_SLOT);
        assert!(saved["content"].get("url").is_none());
        assert_eq!(serde_json::from_value::<Node>(saved).unwrap(),node);
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum StoredContent {
    Terminal { terminal: String },
    Mod {
        #[serde(rename = "mod")]
        name: String,
        #[serde(default)]
        props: Value,
    },
    Context {
        #[serde(alias = "context")]
        tab: String,
        #[serde(default)]
        pane: Option<PaneFrame>,
    },
    Agent { agent: String },
    // Last: a URL field must never steal an existing terminal/mod/context/agent.
    LegacyUrl {
        url: String,
        #[serde(default)]
        tabs: Value,
        #[serde(flatten)]
        rest: serde_json::Map<String, Value>,
    },
}

impl From<StoredContent> for Content {
    fn from(value: StoredContent) -> Self {
        match value {
            StoredContent::Terminal { terminal } => Self::Terminal { terminal },
            StoredContent::Mod { name, props } => Self::Mod { name, props },
            StoredContent::Context { tab, pane } => Self::Context { tab, pane },
            StoredContent::Agent { agent } => Self::Agent { agent },
            StoredContent::LegacyUrl { url, tabs, mut rest } => {
                rest.insert("url".into(),url.into());
                if !tabs.is_null() { rest.insert("tabs".into(),tabs); }
                Self::Mod { name:crate::browser::NATIVE_SLOT.into(), props:json!(rest) }
            }
        }
    }
}
