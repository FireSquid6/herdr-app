//! Plain context metadata only. No active clipboard IPC or website API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub browser: i32,
    pub frame: String,
}

// Process-wide, so a delayed action can never collide with a freshly selected
// tab's first menu token (nor a recreated pane/browser/frame identity).
pub fn next_token() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1
}
use cef::{Browser, CefString, Frame, ImplBrowser, ImplFrame};

pub const MAX_TEXT: usize = 1024 * 1024;
#[derive(Clone)]
pub struct PageContext {
    pub owner: Identity,
    pub generation: u64,
    pub token: u64,
    pub link: String,
    pub selection: String,
    pub editable: bool,
    pub can_copy: bool,
    pub can_cut: bool,
    pub can_paste: bool,
}
impl PageContext {
    /// CEF flags, not website-supplied labels or merely `is_editable`, determine
    /// password/readonly eligibility. This never probes any clipboard.
    pub fn snapshot(owner: Identity, generation: u64, link: String, selection: String, editable: bool, flags: u32) -> Option<Self> {
        if link.len() > 16384 || selection.len() > MAX_TEXT { return None; }
        Some(Self { owner, generation, token: next_token(), link, selection, editable,
            can_copy: flags & 8 != 0, can_cut: editable && flags & 4 != 0,
            can_paste: editable && flags & 16 != 0 })
    }

    pub fn take(pending: &mut Option<Self>, token: &str, owner: &Identity, generation: u64) -> Result<Self, String> {
        let menu = pending.take().ok_or("Page menu expired")?;
        if menu.token.to_string() != token || &menu.owner != owner || menu.generation != generation {
            return Err("Stale page menu".into());
        }
        Ok(menu)
    }
}

pub fn identity(browser: &Browser, frame: &Frame) -> Identity {
    Identity {
        browser: browser.identifier(),
        frame: CefString::from(&frame.identifier()).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn menu() -> PageContext {
        PageContext { owner: Identity { browser: 7, frame: "frame-a".into() }, generation: 3,
            token: next_token(), link: "https://safe.test".into(), selection: "untrusted".into(),
            editable: false, can_copy: true, can_cut: false, can_paste: false }
    }
    #[test]
    fn consuming_menu_binds_native_browser_frame_generation_and_is_one_shot() {
        let original = menu();
        let token = original.token.to_string();
        let owner = original.owner.clone();
        for (current, generation, supplied) in [
            (Identity { browser: 8, ..owner.clone() }, 3, token.clone()),
            (Identity { frame: "frame-b".into(), ..owner.clone() }, 3, token.clone()),
            (owner.clone(), 4, token.clone()),
            (owner.clone(), 3, "not-authorized".into()),
        ] {
            let mut pending = Some(original.clone());
            assert!(PageContext::take(&mut pending, &supplied, &current, generation).is_err());
            assert!(pending.is_none(), "even a stale attempted action consumes the capability");
        }
        let mut pending = Some(original);
        assert_eq!(PageContext::take(&mut pending, &token, &owner, 3).unwrap().link, "https://safe.test");
        assert!(PageContext::take(&mut pending, &token, &owner, 3).is_err());
    }
    #[test]
    fn new_page_or_recreated_owner_never_reuses_a_menu_token() {
        assert_ne!(menu().token, menu().token);
    }
}

#[cfg(test)]
mod edit_flag_tests {
    use super::*;
    fn snapshot(editable: bool, flags: u32) -> PageContext {
        PageContext::snapshot(Identity {browser: 7, frame: "a".into()}, 1,
            "https://safe.test/');run('app.quit')".into(), "plain <script>data</script>".into(), editable, flags).unwrap()
    }
    #[test]
    fn password_copy_and_readonly_mutation_are_not_inferred_from_editable() {
        let password = snapshot(true, 16);
        assert!(!password.can_copy);
        assert!(!password.can_cut);
        assert!(password.can_paste);
        let readonly = snapshot(true, 8);
        assert!(readonly.can_copy);
        assert!(!readonly.can_cut);
        assert!(!readonly.can_paste);
        let selection = snapshot(false, 8 | 4 | 16);
        assert!(selection.can_copy);
        assert!(!selection.can_cut);
        assert!(!selection.can_paste);
        assert_eq!(selection.selection, "plain <script>data</script>");
        assert_eq!(selection.link, "https://safe.test/');run('app.quit')");
    }
    #[test]
    fn oversized_page_metadata_is_rejected() {
        let owner = Identity {browser: 7, frame: "a".into()};
        assert!(PageContext::snapshot(owner.clone(), 1, "x".repeat(16385), String::new(), false, 0).is_none());
        assert!(PageContext::snapshot(owner, 1, String::new(), "x".repeat(MAX_TEXT + 1), false, 0).is_none());
    }
}
