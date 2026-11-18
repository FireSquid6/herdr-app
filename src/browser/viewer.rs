//! A workspace's pane viewers share one dedicated core connection/lease identity.
//! Only live runtime tasks retain the connection: this holder never starts core
//! or keeps a viewer attached merely because a saved workspace exists.
use crate::hc::Core;
use anyhow::Result;
use std::{collections::VecDeque, sync::{Arc, Mutex, Weak}};

#[derive(Default)]
pub struct Viewer {
    connection: Mutex<Weak<Core>>,
    stopped: Mutex<VecDeque<String>>,
}

impl Viewer {
    /// Publish only a confirmed stop receipt, keyed by engine instance rather
    /// than PID/generation so a later explicit restart cannot inherit it.
    pub fn observed_stop(&self, instance: &str) {
        if instance.is_empty() { return; }
        let mut stopped = self.stopped.lock().unwrap();
        if !stopped.iter().any(|old| old == instance) {
            if stopped.len() == 16 { stopped.pop_front(); }
            stopped.push_back(instance.to_owned());
        }
    }
    pub fn was_stopped(&self, instance: &str) -> bool {
        !instance.is_empty() && self.stopped.lock().unwrap().iter().any(|old| old == instance)
    }

    pub fn connect(&self) -> Result<Arc<Core>> {
        let mut slot = self.connection.lock().unwrap();
        if let Some(core) = slot.upgrade().filter(|core| !core.is_closed()) {
            return Ok(core);
        }
        // Serialize only this workspace's first dial, off the UI thread. A
        // second pane must not race into another controller identity.
        let core = Core::connect_browser()?;
        *slot = Arc::downgrade(&core);
        Ok(core)
    }
}

#[cfg(test)]
mod tests {
    use super::Viewer;
    #[test]
    fn confirmed_stop_is_workspace_local_and_never_applies_to_a_new_instance() {
        let workspace = Viewer::default();
        let other = Viewer::default();
        assert!(!workspace.was_stopped("instance-a"));
        workspace.observed_stop("instance-a");
        assert!(workspace.was_stopped("instance-a"));
        assert!(!workspace.was_stopped("instance-b"));
        assert!(!other.was_stopped("instance-a"));
        workspace.observed_stop("instance-b");
        assert!(workspace.was_stopped("instance-a"));
        assert!(workspace.was_stopped("instance-b"));
        workspace.observed_stop("");
        assert!(!workspace.was_stopped(""));
    }
}
