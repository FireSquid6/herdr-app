//! Pure favicon authorization/lifecycle. Asset strings originate only from the
//! native decoded-PNG writer, never from page-provided URLs or paths.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FaviconState {
    pub asset: Option<String>,
    epoch: u64,
    serial: u64,
    pending: Option<FaviconRequest>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaviconRequest {
    pub url: String,
    epoch: u64,
    serial: u64,
}

impl FaviconState {
    pub fn document_changed(&mut self) -> Option<String> {
        self.epoch += 1;
        self.pending = None;
        self.asset.take()
    }
    /// Even an empty/disallowed candidate list revokes the previous request.
    pub fn request(&mut self, url: Option<String>) -> (Option<FaviconRequest>, Option<String>) {
        self.serial += 1;
        self.pending = url.map(|url|FaviconRequest { url, epoch: self.epoch, serial: self.serial });
        (self.pending.clone(), self.asset.take())
    }
    #[cfg(test)]
    pub fn pending_url(&self) -> Option<&str> { self.pending.as_ref().map(|request|request.url.as_str()) }
    pub fn accepts(&self, request: &FaviconRequest) -> bool {
        self.pending.as_ref() == Some(request)
    }
    /// None is a failed current download. Stale failures cannot clear a newer
    /// icon, just as stale successes cannot install an old document's icon.
    pub fn complete(&mut self, request: &FaviconRequest, asset: Option<String>) -> Result<Option<String>, ()> {
        if !self.accepts(request) { return Err(()); }
        self.pending = None;
        Ok(std::mem::replace(&mut self.asset, asset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn valid_icon_to_new_document_and_no_icon_retires_the_old_asset() {
        let mut state = FaviconState::default();
        let (request, retired) = state.request(Some("https://fixture/icon.png".into()));
        assert!(retired.is_none());
        state.complete(&request.unwrap(), Some("native/old.png".into())).unwrap();
        assert_eq!(state.document_changed(), Some("native/old.png".into()));
        assert!(state.asset.is_none());
        assert_eq!(state.request(None), (None, None));
        assert!(state.asset.is_none());
    }
    #[test]
    fn failed_current_icon_clears_it_and_stale_reply_cannot_resurrect_it() {
        let mut state = FaviconState::default();
        let (old, _) = state.request(Some("https://fixture/icon.png".into()));
        let old = old.unwrap();
        state.document_changed();
        let (new, _) = state.request(Some("https://fixture/icon.png".into()));
        let new = new.unwrap();
        assert!(state.complete(&old, Some("native/stale.png".into())).is_err());
        state.complete(&new, None).unwrap();
        assert!(state.asset.is_none());
        assert!(state.complete(&old, Some("native/stale.png".into())).is_err());
    }
    #[test]
    fn stale_success_or_failure_cannot_replace_or_clear_a_new_icon() {
        let mut state = FaviconState::default();
        let (old, _) = state.request(Some("https://fixture/old.png".into()));
        let old = old.unwrap();
        let (current, _) = state.request(Some("https://fixture/current.png".into()));
        state.complete(&current.unwrap(), Some("native/current.png".into())).unwrap();
        assert!(state.complete(&old, None).is_err());
        assert!(state.complete(&old, Some("native/stale.png".into())).is_err());
        assert_eq!(state.asset.as_deref(), Some("native/current.png"));
    }
    #[test]
    fn empty_or_replaced_icon_list_revokes_even_a_same_document_request() {
        let mut state = FaviconState::default();
        let (old, _) = state.request(Some("https://fixture/icon.png".into()));
        let old = old.unwrap();
        state.request(None);
        assert!(state.complete(&old, Some("native/stale.png".into())).is_err());
        let (current, _) = state.request(Some(old.url.clone()));
        let current = current.unwrap();
        state.complete(&current, Some("native/current.png".into())).unwrap();
        assert_eq!(state.request(None).1, Some("native/current.png".into()));
        assert!(state.asset.is_none());
    }
}
