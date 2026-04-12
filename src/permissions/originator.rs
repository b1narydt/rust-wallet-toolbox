//! Originator normalization utilities.
//!
//! Translates the TypeScript `normalizeOriginator` and `prepareOriginator`
//! logic from `WalletPermissionsManager`. Handles URL parsing, default port
//! stripping, hostname lowercasing, and legacy lookup value generation.

/// Normalize an originator string to a canonical form.
///
/// - Parses as URL (prepending `https://` if no scheme present)
/// - Lowercases the hostname
/// - Strips default ports (80 for HTTP, 443 for HTTPS)
/// - Removes trailing path/query
/// - Falls back to lowercase trimmed string if URL parsing fails
pub fn normalize_originator(originator: &str) -> String {
    let trimmed = originator.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Check if the string already has a scheme
    let has_scheme = {
        let lower = trimmed.to_lowercase();
        lower.starts_with("http://") || lower.starts_with("https://")
    };

    let candidate = if has_scheme {
        trimmed.to_string()
    } else {
        format!("https://{}", trimmed)
    };

    // Attempt to parse as URL
    match parse_url_parts(&candidate) {
        Some((scheme, hostname, port)) => {
            let hostname_lower = hostname.to_lowercase();

            // Wrap IPv6 addresses in brackets
            let needs_brackets = hostname_lower.contains(':');
            let base_host = if needs_brackets {
                format!("[{}]", hostname_lower)
            } else {
                hostname_lower
            };

            // Determine if the port is a default port that should be stripped
            let default_port = match scheme.as_str() {
                "http" => Some("80"),
                "https" => Some("443"),
                _ => None,
            };

            if let Some(p) = &port {
                if let Some(dp) = default_port {
                    if p == dp {
                        // Strip default port
                        return base_host;
                    }
                }
                format!("{}:{}", base_host, p)
            } else {
                base_host
            }
        }
        None => {
            // Fall back to conservative lowercase trim
            trimmed.to_lowercase()
        }
    }
}

/// Generate all tag-lookup values for an originator.
///
/// Returns a deduplicated list containing the normalized value and the original
/// trimmed value, used for backwards-compatible token lookups.
pub fn build_originator_lookup_values(originator: &str) -> Vec<String> {
    let trimmed = originator.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let normalized = normalize_originator(trimmed);
    let mut values = Vec::new();
    values.push(trimmed.to_string());
    if normalized != trimmed {
        values.push(normalized);
    }
    values.dedup();
    values
}

/// Compare two originator strings after normalization.
pub fn is_admin_originator(originator: &str, admin_originator: &str) -> bool {
    let norm_orig = normalize_originator(originator);
    let norm_admin = normalize_originator(admin_originator);
    !norm_orig.is_empty() && !norm_admin.is_empty() && norm_orig == norm_admin
}

// ---------------------------------------------------------------------------
// Simple URL parser (avoids pulling in the `url` crate)
// ---------------------------------------------------------------------------

/// Parse scheme, hostname, and optional port from a URL string.
///
/// Returns `None` if parsing fails.
fn parse_url_parts(url: &str) -> Option<(String, String, Option<String>)> {
    // Find scheme
    let scheme_end = url.find("://")?;
    let scheme = url[..scheme_end].to_lowercase();
    let rest = &url[scheme_end + 3..];

    // Strip path/query/fragment
    let authority = rest
        .split('/')
        .next()
        .unwrap_or(rest)
        .split('?')
        .next()
        .unwrap_or(rest)
        .split('#')
        .next()
        .unwrap_or(rest);

    if authority.is_empty() {
        return None;
    }

    // Strip userinfo (user:pass@)
    let host_port = if let Some(at_pos) = authority.rfind('@') {
        &authority[at_pos + 1..]
    } else {
        authority
    };

    // Handle IPv6 brackets
    if host_port.starts_with('[') {
        // IPv6: [::1]:port or [::1]
        if let Some(bracket_end) = host_port.find(']') {
            let hostname = &host_port[1..bracket_end];
            let after_bracket = &host_port[bracket_end + 1..];
            let port = if let Some(p) = after_bracket.strip_prefix(':') {
                if p.is_empty() {
                    None
                } else {
                    Some(p.to_string())
                }
            } else {
                None
            };
            return Some((scheme, hostname.to_string(), port));
        }
        return None;
    }

    // Regular hostname:port
    // Find the LAST colon (to handle ambiguity, though non-bracketed IPv6 is invalid in URLs)
    if let Some(colon_pos) = host_port.rfind(':') {
        let maybe_port = &host_port[colon_pos + 1..];
        // Only treat as port if all digits
        if !maybe_port.is_empty() && maybe_port.chars().all(|c| c.is_ascii_digit()) {
            let hostname = &host_port[..colon_pos];
            if hostname.is_empty() {
                return None;
            }
            return Some((scheme, hostname.to_string(), Some(maybe_port.to_string())));
        }
    }

    Some((scheme, host_port.to_string(), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_originator_strips_port_443() {
        assert_eq!(
            normalize_originator("https://example.com:443"),
            "example.com"
        );
        assert_eq!(normalize_originator("example.com:443"), "example.com");
    }

    #[test]
    fn test_normalize_originator_strips_port_80() {
        assert_eq!(normalize_originator("http://example.com:80"), "example.com");
    }

    #[test]
    fn test_normalize_originator_preserves_custom_port() {
        assert_eq!(normalize_originator("example.com:8080"), "example.com:8080");
        assert_eq!(
            normalize_originator("https://example.com:9090"),
            "example.com:9090"
        );
    }

    #[test]
    fn test_normalize_originator_lowercases() {
        assert_eq!(normalize_originator("EXAMPLE.COM"), "example.com");
        assert_eq!(normalize_originator("https://MyDomain.Org"), "mydomain.org");
    }

    #[test]
    fn test_normalize_originator_empty() {
        assert_eq!(normalize_originator(""), "");
        assert_eq!(normalize_originator("  "), "");
    }

    #[test]
    fn test_normalize_originator_trims_whitespace() {
        assert_eq!(normalize_originator("  example.com  "), "example.com");
    }

    #[test]
    fn test_is_admin_originator() {
        assert!(is_admin_originator("example.com", "example.com"));
        assert!(is_admin_originator("EXAMPLE.COM", "example.com"));
        assert!(is_admin_originator(
            "https://example.com:443",
            "example.com"
        ));
        assert!(!is_admin_originator("other.com", "example.com"));
        assert!(!is_admin_originator("", "example.com"));
    }

    #[test]
    fn test_build_originator_lookup_values() {
        let values = build_originator_lookup_values("EXAMPLE.COM");
        assert!(values.contains(&"EXAMPLE.COM".to_string()));
        assert!(values.contains(&"example.com".to_string()));

        let empty = build_originator_lookup_values("");
        assert!(empty.is_empty());
    }
}
