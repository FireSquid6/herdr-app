//! Trusted mod composition boundary. Page pixels never enter the mod VM; page
//! strings cross only as plain JSON values, with no HTML or code interpretation.
use gpui::{AnyElement, ParentElement, Styled, div, prelude::FluentBuilder};
#[cfg(all(feature = "browser", target_os = "linux"))]
use gpui::{Context, IntoElement, Render, WeakEntity, Window};
use gpui_shell::{
    ArgumentDescriptor, ArgumentSchema, COMPONENT_REGISTRY_API_VERSION, ComponentArgument,
    ComponentDescriptor, ComponentMaterializer, ComponentPayload, ComponentRegistry,
    ConstructorDescriptor, MaterializeRequest,
};
use serde_json::Value;
use std::{cell::RefCell, collections::HashMap, sync::Arc};

thread_local! {
    static STATES: RefCell<HashMap<String, Value>> = RefCell::new(HashMap::new());
    #[cfg(all(feature = "browser", target_os = "linux"))]
    static PAGES: RefCell<HashMap<String, WeakEntity<NativePage>>> = RefCell::new(HashMap::new());
}
pub fn state(pane: &str) -> Value {
    STATES.with_borrow(|states| states.get(pane).cloned().unwrap_or(Value::Null))
}
pub fn publish_state(pane: &str, state: Value) {
    STATES.with_borrow_mut(|states| {
        states.insert(pane.into(), state);
    });
}
#[cfg(not(all(feature = "browser", target_os = "linux")))]
pub fn retain_states(panes: &std::collections::HashSet<String>) {
    STATES.with_borrow_mut(|states| states.retain(|pane, _| panes.contains(pane)));
}
#[cfg(all(feature = "browser", target_os = "linux"))]
pub fn publish(pane: &str, state: Value, view: WeakEntity<NativePage>) {
    publish_state(pane, state);
    PAGES.with_borrow_mut(|pages| {
        pages.insert(pane.into(), view);
    });
}
#[cfg(all(feature = "browser", target_os = "linux"))]
pub fn remove_state(pane: &str) {
    STATES.with_borrow_mut(|states| {
        states.remove(pane);
    });
}
#[cfg(all(feature = "browser", target_os = "linux"))]
pub fn remove(pane: &str) {
    remove_state(pane);
    PAGES.with_borrow_mut(|pages| {
        pages.remove(pane);
    });
}
#[cfg(all(feature = "browser", target_os = "linux"))]
pub struct NativePage(pub WeakEntity<crate::browser::BrowserPane>);
#[cfg(all(feature = "browser", target_os = "linux"))]
impl Render for NativePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0
            .update(cx, |view, cx| view.render_page(cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}
struct Page;
impl ComponentMaterializer for Page {
    fn materialize(&self, request: MaterializeRequest<'_>) -> anyhow::Result<AnyElement> {
        let pane = request
            .payload()
            .downcast_ref::<String>()
            .expect("validated pane")
            .clone();
        #[cfg(all(feature = "browser", target_os = "linux"))]
        let content = PAGES.with_borrow(|pages| {
            pages
                .get(&pane)
                .and_then(WeakEntity::upgrade)
                .map(IntoElement::into_any_element)
        });
        #[cfg(not(all(feature = "browser", target_os = "linux")))]
        let content: Option<AnyElement> = {
            let _ = pane;
            None
        };
        request.finish(
            div()
                .size_full()
                .when_some(content, |el, page| el.child(page)),
        )
    }
}
pub fn components() -> gpui_shell::FrozenComponentRegistry {
    let mut registry =
        ComponentRegistry::new(COMPONENT_REGISTRY_API_VERSION, "herdr-browser").expect("registry");
    registry.register(ComponentDescriptor::new("BrowserPage", Arc::new(Page))
        .with_documentation("Native page pixels and input only; trusted mods own all surrounding UI. Use once per browser pane. Unavailable panes render empty.")
        .with_constructors(vec![ConstructorDescriptor::new("BrowserPage", vec![ArgumentDescriptor::new("pane", ArgumentSchema::String)], |args| {
            match args.first() {
                Some(ComponentArgument::String(pane)) => Ok(ComponentPayload::new(pane.clone())),
                _ => Err("BrowserPage requires a pane ID".into()),
            }
        })])).expect("BrowserPage registration");
    registry.freeze().expect("browser registry")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_types_match_the_native_surface_module() {
        let declarations = gpui_shell::type_declarations(&components());
        assert!(declarations.contains("declare module \"herdr-browser\""));
        assert!(
            declarations
                .contains("export const BrowserPage: { (pane: string): BrowserPageElement; new(")
        );
        assert!(declarations.contains("import(\"herdr-browser\").BrowserPageElement"));
    }
    #[cfg(not(all(feature = "browser", target_os = "linux")))]
    #[test]
    fn browser_free_metadata_is_purged_even_without_an_instantiated_copy() {
        publish_state("hidden-browser", serde_json::json!({"available":false}));
        retain_states(&["hidden-browser".into()].into());
        assert_eq!(state("hidden-browser")["available"], false);
        retain_states(&Default::default());
        assert_eq!(state("hidden-browser"), Value::Null);
    }
}
