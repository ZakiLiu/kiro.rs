use url::Url;

pub const ALLOWED_IDP_HOST_SUFFIXES: &[&str] = &[
    "login.microsoftonline.com",
    "login.microsoftonline.us",
    "login.chinacloudapi.cn",
    "login.microsoftonline.de",
];

pub fn validate_external_idp_endpoint(
    url_str: &str,
    allowed_suffixes: &[&str],
) -> Result<(), String> {
    let url = Url::parse(url_str).map_err(|e| format!("invalid URL: {e}"))?;

    if url.scheme() != "https" {
        return Err(format!(
            "external IdP endpoint rejected: scheme must be https, got '{}'",
            url.scheme()
        ));
    }

    let host = url
        .host_str()
        .ok_or("external IdP endpoint rejected: no host")?;

    if host.parse::<std::net::Ipv4Addr>().is_ok()
        || host.starts_with('[')
        || host.parse::<std::net::Ipv6Addr>().is_ok()
    {
        return Err(format!(
            "external IdP endpoint rejected: IP literal not allowed: {host}"
        ));
    }

    let host_lower = host.to_ascii_lowercase();
    let matched = allowed_suffixes.iter().any(|suffix| {
        host_lower == *suffix || host_lower.ends_with(&format!(".{suffix}"))
    });

    if !matched {
        return Err(format!(
            "external IdP endpoint rejected: host '{host}' not in allow-list"
        ));
    }

    Ok(())
}

pub fn validate_with_default_allowlist(url_str: &str) -> Result<(), String> {
    validate_external_idp_endpoint(url_str, ALLOWED_IDP_HOST_SUFFIXES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_valid_endpoint() {
        assert!(validate_with_default_allowlist(
            "https://login.microsoftonline.com/tenant/oauth2/v2.0/token"
        )
        .is_ok());
        assert!(validate_with_default_allowlist(
            "https://login.microsoftonline.us/tenant/oauth2/v2.0/token"
        )
        .is_ok());
        assert!(validate_with_default_allowlist(
            "https://login.chinacloudapi.cn/tenant/oauth2/v2.0/token"
        )
        .is_ok());
    }

    #[test]
    fn test_reject_http() {
        let result = validate_with_default_allowlist(
            "http://login.microsoftonline.com/tenant/token",
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("scheme must be https"));
    }

    #[test]
    fn test_reject_ip_literal() {
        assert!(validate_with_default_allowlist("https://192.168.1.1/token").is_err());
        assert!(validate_with_default_allowlist("https://127.0.0.1/token").is_err());
        assert!(validate_with_default_allowlist("https://[::1]/token").is_err());
    }

    #[test]
    fn test_reject_subdomain_spoof() {
        assert!(validate_with_default_allowlist(
            "https://login.microsoftonline.com.attacker.com/token"
        )
        .is_err());
        assert!(validate_with_default_allowlist(
            "https://evil-login.microsoftonline.com.evil.com/token"
        )
        .is_err());
    }

    #[test]
    fn test_reject_non_allowlisted_host() {
        assert!(
            validate_with_default_allowlist("https://accounts.google.com/token").is_err()
        );
        assert!(validate_with_default_allowlist("https://evil.com/token").is_err());
    }

    #[test]
    fn test_custom_allowlist() {
        let custom = &["custom-idp.example.com"];
        assert!(
            validate_external_idp_endpoint("https://custom-idp.example.com/token", custom)
                .is_ok()
        );
        assert!(validate_external_idp_endpoint(
            "https://sub.custom-idp.example.com/token",
            custom
        )
        .is_ok());
        assert!(validate_external_idp_endpoint(
            "https://login.microsoftonline.com/token",
            custom
        )
        .is_err());
    }
}
