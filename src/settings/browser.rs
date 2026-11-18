//! Address interpretation shared by browser commands and the native address field.

pub const DEFAULT_URL: &str = "https://www.google.com";
pub const DEFAULT_SEARCH: &str = "https://www.google.com/search?q={query}";

pub fn target(input: &str, template: &str) -> String {
    let input = input.trim();
    if input.is_empty() { return DEFAULT_URL.into(); }
    if let Some((scheme, rest)) = input.split_once("://") {
        if scheme.eq_ignore_ascii_case("http") { return format!("http://{rest}"); }
        if scheme.eq_ignore_ascii_case("https") { return format!("https://{rest}"); }
    }
    if !input.contains(char::is_whitespace) && !input.contains("://") {
        let authority = input.split(['/', '?', '#']).next().unwrap_or("");
        let parsed = if let Some(v6) = authority.strip_prefix('[') {
            v6.split_once(']').and_then(|(host, tail)| {
                host.parse::<std::net::Ipv6Addr>().ok()?;
                let port = if tail.is_empty() { None } else { Some(tail.strip_prefix(':')?) };
                Some((host, port, true))
            })
        } else {
            let (host, port) = authority.split_once(':').map_or((authority, None), |(h, p)| (h, Some(p)));
            let host_ok = !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b));
            host_ok.then_some((host, port, false))
        };
        if let Some((host, port, ipv6)) = parsed {
            let valid_port = port.is_none_or(|p| p.parse::<u16>().is_ok_and(|p| p != 0));
            let loopback = host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1";
            if valid_port && (loopback || port.is_some() || ipv6 || host.contains('.')) {
                // Bare explicit ports are typically development HTTP services.
                // Explicit https:// is preserved above, never downgraded.
                let scheme = if loopback || port.is_some() { "http" } else { "https" };
                return format!("{scheme}://{input}");
            }
        }
    }
    let query: String = input.bytes().map(|b| {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() }
        else { format!("%{b:02X}") }
    }).collect();
    let template = if (template.starts_with("https://") || template.starts_with("http://")) && template.contains("{query}") {
        template
    } else { DEFAULT_SEARCH };
    template.replace("{query}", &query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_searches_use_only_web_urls_and_encode_queries() {
        assert_eq!(target("example.com/path", "https://search.example/?q={query}"), "https://example.com/path");
        assert_eq!(target("cats & dogs", "https://search.example/?q={query}"), "https://search.example/?q=cats%20%26%20dogs");
        assert_eq!(target("javascript:alert(1)", "https://search.example/?q={query}"), "https://search.example/?q=javascript%3Aalert%281%29");
        assert_eq!(target("hello world", "file:///{query}"), "https://www.google.com/search?q=hello%20world");
        assert_eq!(target("hello world", "https://example.com/"), "https://www.google.com/search?q=hello%20world");
        assert_eq!(target("http://localhost:8000", ""), "http://localhost:8000");
    }

    #[test]
    fn bare_development_services_stay_http_including_ipv6() {
        for input in ["localhost:3000", "localhost:3000/path?q=dev", "127.0.0.1:8080", "[::1]:3000/path", "localhost", "[::1]", "devbox:8000"] {
            assert_eq!(target(input, DEFAULT_SEARCH), format!("http://{input}"));
        }
        assert_eq!(target("https://localhost:3000", DEFAULT_SEARCH), "https://localhost:3000");
        assert_eq!(target("HTTPS://example.com", DEFAULT_SEARCH), "https://example.com");
    }

    #[test]
    fn invalid_ports_and_nonweb_schemes_are_searches_not_executed_urls() {
        for input in ["localhost:bad", "localhost:70000", "file:///etc/passwd", "[::1]:0", "mailto:user@example.com"] {
            assert!(target(input, DEFAULT_SEARCH).starts_with("https://www.google.com/search?q="), "{input}");
        }
    }
}
