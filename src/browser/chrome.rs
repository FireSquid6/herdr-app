//! Trusted chrome composition. Only plain metadata enters the mod VM; streamed
//! website pixels and input remain native entities with no website/mod bridge.
use gpui::{prelude::*, *};
use gpui_shell::{ArgumentDescriptor, ArgumentSchema, COMPONENT_REGISTRY_API_VERSION,
    ComponentArgument, ComponentDescriptor, ComponentMaterializer, ComponentPayload,
    ComponentRegistry, ConstructorDescriptor, MaterializeRequest};
use serde_json::Value;
use std::{cell::RefCell, collections::HashMap, sync::Arc};

thread_local! {
    static STATES: RefCell<HashMap<String, Value>> = RefCell::new(HashMap::new());
    static PAGES: RefCell<HashMap<String, WeakEntity<NativePage>>> = RefCell::new(HashMap::new());
}

pub fn state(pane: &str) -> Value {
    STATES.with_borrow(|states| states.get(pane).cloned().unwrap_or(Value::Null))
}
pub fn publish(pane: &str, state: Value, page: WeakEntity<NativePage>) -> bool {
    let changed = STATES.with_borrow_mut(|states| {
        if states.get(pane) == Some(&state) { false }
        else { states.insert(pane.into(), state); true }
    });
    PAGES.with_borrow_mut(|pages| { pages.insert(pane.into(), page); });
    changed
}
pub fn remove(pane: &str) {
    STATES.with_borrow_mut(|states| { states.remove(pane); });
    PAGES.with_borrow_mut(|pages| { pages.remove(pane); });
}

pub struct NativePage(pub WeakEntity<super::panel::Panel>);
impl Render for NativePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.update(cx, |panel, cx| panel.render_page(cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}
struct Page;
impl ComponentMaterializer for Page {
    fn materialize(&self, request: MaterializeRequest<'_>) -> anyhow::Result<AnyElement> {
        let pane = request.payload().downcast_ref::<String>().expect("validated pane");
        let content = PAGES.with_borrow(|pages| pages.get(pane)
            .and_then(WeakEntity::upgrade).map(IntoElement::into_any_element));
        request.finish(div().size_full().when_some(content, |element, page| element.child(page)))
    }
}
pub fn components() -> gpui_shell::FrozenComponentRegistry {
    let mut registry = ComponentRegistry::new(COMPONENT_REGISTRY_API_VERSION, "herdr-browser").expect("browser registry");
    registry.register(ComponentDescriptor::new("BrowserPage", Arc::new(Page))
        .with_documentation("Native core-owned page/DevTools surfaces. Trusted mods own surrounding chrome; no website code runs in the mod VM.")
        .with_constructors(vec![ConstructorDescriptor::new("BrowserPage", vec![ArgumentDescriptor::new("pane", ArgumentSchema::String)], |args| {
            match args.first() {
                Some(ComponentArgument::String(pane)) => Ok(ComponentPayload::new(pane.clone())),
                _ => Err("BrowserPage requires a pane ID".into()),
            }
        })])).expect("BrowserPage registration");
    registry.freeze().expect("frozen browser registry")
}
