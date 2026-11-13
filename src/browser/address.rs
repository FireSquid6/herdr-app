//! Human address-bar interpretation only; core navigation remains strict.
use url::{Host, Url};

pub(super) fn address_url(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if input.eq_ignore_ascii_case("about:blank") {
        return Some("about:blank".into());
    }
    let parsed = Url::parse(input).ok();
    if !input.chars().any(char::is_control) {
        if let Some(url) = &parsed {
            if matches!(url.scheme(), "http" | "https") && url.host().is_some() {
                return Some(url.to_string());
            }
        }
    }
    // Preserve unsupported explicit schemes for core's existing rejection,
    // rather than silently leaking local file paths or data URLs to Google.
    let scheme = input
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    if matches!(
        scheme.as_deref(),
        Some(
            "file"
                | "javascript"
                | "data"
                | "about"
                | "chrome"
                | "chrome-extension"
                | "devtools"
                | "mailto"
                | "tel"
                | "magnet"
        )
    ) || (parsed.is_some()
        && input
            .split_once(':')
            .is_some_and(|(_, rest)| rest.starts_with("//")))
    {
        return Some(input.to_owned());
    }
    let site = input.strip_prefix("//").unwrap_or(input);
    if !site.starts_with('/') && !input.chars().any(char::is_whitespace) {
        if let Ok(url) = Url::parse(&format!("https://{site}")) {
            // Bare email addresses and user:password text are searches, not
            // implicit URL credentials. Explicit HTTP(S) URLs remain supported.
            if url.username().is_empty() && url.password().is_none() && site_host(&url, site) {
                return Some(url.to_string());
            }
        }
    }
    let mut search = Url::parse("https://www.google.com/search").expect("fixed search URL");
    search.query_pairs_mut().append_pair("q", input);
    Some(search.into())
}

fn site_host(url: &Url, site: &str) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => {
            let host = host.strip_suffix('.').unwrap_or(host);
            host == "localhost"
                || (host.contains('.')
                    && host.len() <= 253
                    && host.split('.').all(|label| {
                        !label.is_empty()
                            && label.len() <= 63
                            && !label.starts_with('-')
                            && !label.ends_with('-')
                            && label
                                .bytes()
                                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                    }))
        }
        // Don't mistake decimal search text such as 3.14 for WHATWG's legacy
        // shortened IPv4 syntax. Explicit HTTP(S) URLs still use normal parsing.
        Some(Host::Ipv4(_)) => site
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .parse::<std::net::Ipv4Addr>()
            .is_ok(),
        Some(Host::Ipv6(_)) => true,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::address_url;

    #[test]
    fn bare_sites_gain_https_without_losing_path_port_query_or_fragment() {
        for (input, expected) in [
            ("google.com", "https://google.com/"),
            ("  GOOGLE.com  ", "https://google.com/"),
            (
                "example.com/docs?q=rust#intro",
                "https://example.com/docs?q=rust#intro",
            ),
            ("example.com:8443/path", "https://example.com:8443/path"),
            (
                "example.com:8443/?next=https://other.example",
                "https://example.com:8443/?next=https://other.example",
            ),
            ("//example.com/path", "https://example.com/path"),
            ("localhost:3000", "https://localhost:3000/"),
            ("127.0.0.1:8080", "https://127.0.0.1:8080/"),
            ("[::1]:8080/test", "https://[::1]:8080/test"),
            ("bücher.de", "https://xn--bcher-kva.de/"),
        ] {
            assert_eq!(address_url(input).as_deref(), Some(expected), "{input}");
        }
    }

    #[test]
    fn explicit_urls_remain_urls_including_http_and_blank() {
        for (input, expected) in [
            ("http://localhost:3000/test", "http://localhost:3000/test"),
            ("HTTPS://Example.com/path", "https://example.com/path"),
            ("https://example.com/a b", "https://example.com/a%20b"),
            (
                "https://example.com/?q=a%26b#part",
                "https://example.com/?q=a%26b#part",
            ),
            ("about:blank", "about:blank"),
            ("ABOUT:blank", "about:blank"),
        ] {
            assert_eq!(address_url(input).as_deref(), Some(expected), "{input}");
        }
    }

    #[test]
    fn other_text_becomes_a_single_encoded_google_search_query() {
        for (input, query) in [
            ("  how to use Rust  ", "how+to+use+Rust"),
            (
                "cats & dogs #1 /? café",
                "cats+%26+dogs+%231+%2F%3F+caf%C3%A9",
            ),
            ("C++", "C%2B%2B"),
            ("rust:lifetimes", "rust%3Alifetimes"),
            ("person@example.com", "person%40example.com"),
            ("3.14", "3.14"),
            ("hello\nworld", "hello%0Aworld"),
            ("example.com:99999", "example.com%3A99999"),
            ("not..a.site", "not..a.site"),
        ] {
            assert_eq!(
                address_url(input),
                Some(format!("https://www.google.com/search?q={query}")),
                "{input}"
            );
        }
    }

    #[test]
    fn empty_address_is_a_noop_and_unsupported_explicit_urls_are_not_leaked_to_search() {
        assert_eq!(address_url(" \n\t "), None);
        // Core rejects these schemes as before; do not execute them or send their
        // contents (potentially private local paths/data) to a search service.
        for input in [
            "file:///tmp/private.html",
            "javascript:alert(1)",
            "data:text/plain,private",
            "chrome://settings",
            "about:config",
            "ftp://example.com",
            "custom.scheme://resource",
        ] {
            assert_eq!(address_url(input).as_deref(), Some(input));
        }
    }
}
