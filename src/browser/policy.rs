use std::path::{Path, PathBuf};

/// Every byte is encoded, including separators, dots and empty workspace IDs.
/// This is reversible and collision-free; never join an untrusted workspace ID.
pub fn profile_path(root: &Path, workspace: &str) -> PathBuf {
    let encoded: String = workspace.bytes().map(|b| format!("{b:02x}")).collect();
    root.join(format!("workspace-{encoded}"))
}
pub fn local_allowed(remote: bool) -> bool {
    !remote
}

/// Main/subframe navigation cannot expose local files or external protocols.
pub fn allowed_navigation(url: &str) -> bool {
    url == "about:blank" || url.starts_with("https://") || url.starts_with("http://")
}

/// Opaque/data resources remain subject to Chromium's origin/CSP checks. This
/// expands resource loading only, never user or frame navigation permissions.
pub fn allowed_resource(url: &str) -> bool {
    allowed_navigation(url)
        || ["ws://", "wss://", "data:", "blob:"]
            .iter()
            .any(|scheme| url.starts_with(scheme))
}

#[cfg(test)]
mod safety_tests {
    use super::*;
    #[test]
    fn profiles_cannot_traverse_or_collide() {
        let root = Path::new("/isolated");
        assert_eq!(
            profile_path(root, "../a"),
            PathBuf::from("/isolated/workspace-2e2e2f61")
        );
        assert_ne!(profile_path(root, "A"), profile_path(root, "a"));
        assert_eq!(
            profile_path(root, ""),
            PathBuf::from("/isolated/workspace-")
        );
    }
    #[test]
    fn remote_is_blocked_without_fallback() {
        assert!(!local_allowed(true));
        assert!(local_allowed(false));
    }
    #[test]
    fn web_only() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailto:user@example.com",
            "chrome://settings",
            "devtools://foo",
        ] {
            assert!(!allowed_navigation(url));
        }
        assert!(allowed_navigation("http://127.0.0.1:8000/"));
        assert!(allowed_navigation("https://www.google.com/"));
        assert!(allowed_navigation("about:blank"));
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    #[test]
    fn web_resources_are_not_navigation_capabilities() {
        for url in [
            "data:image/png;base64,AA==",
            "blob:https://example.com/id",
            "ws://localhost:8000",
            "wss://example.com/live",
        ] {
            assert!(allowed_resource(url));
            assert!(!allowed_navigation(url));
        }
        for url in [
            "file:///etc/passwd",
            "devtools://foo",
            "chrome://settings",
            "javascript:alert(1)",
            "mailto:user@example.com",
        ] {
            assert!(!allowed_resource(url));
            assert!(!allowed_navigation(url));
        }
    }
}

/// Logs never include URL credentials, paths, queries or fragments.
pub fn diagnostic_origin(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return "non-web/internal".into();
    };
    if !["http", "https", "ws", "wss"].contains(&scheme) {
        return "non-web/internal".into();
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    format!(
        "{scheme}://{}",
        host.chars()
            .filter(|c| !c.is_control())
            .take(256)
            .collect::<String>()
    )
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    #[test]
    fn logs_only_web_origin_without_credentials_or_tokens() {
        assert_eq!(
            diagnostic_origin("https://user:secret@example.com:443/private?token=abc#x"),
            "https://example.com:443"
        );
        assert_eq!(
            diagnostic_origin("data:text/html,secret"),
            "non-web/internal"
        );
    }
}
