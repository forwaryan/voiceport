//! Read an explicit HTTPS proxy without changing the process environment.
use core_foundation::{
    base::{CFType, TCFType},
    dictionary::{CFDictionary, CFDictionaryRef},
    number::CFNumber,
    string::CFString,
};
use std::net::Ipv6Addr;

#[link(name = "CFNetwork", kind = "framework")]
unsafe extern "C" {
    fn CFNetworkCopySystemProxySettings() -> CFDictionaryRef;
}

/// Pass the result to `TranscriptionClient::new_with_proxy` for URL validation.
/// An explicitly set value, including an empty/invalid one, takes precedence;
/// invalid configuration must fail validation rather than silently fall back.
/// This function never reads credentials or applies PAC/auto-discovery rules.
pub fn https_proxy() -> Option<String> {
    if let Some(value) = std::env::var_os("VOICEPORT_HTTPS_PROXY") {
        // Non-Unicode environment bytes cannot form the required URL. Keep an
        // explicit invalid value so the core rejects it without exposing it.
        return Some(value.into_string().unwrap_or_default());
    }
    system_https_proxy()
}

fn system_https_proxy() -> Option<String> {
    let raw = unsafe { CFNetworkCopySystemProxySettings() };
    if raw.is_null() {
        return None;
    }
    // Copy returns an owned dictionary of property-list keys and values. This
    // wrapper releases it on every exit; downcasts check each value's CF type.
    let settings = unsafe { CFDictionary::<CFString, CFType>::wrap_under_create_rule(raw) };
    let number = |key: &str| {
        settings
            .find(CFString::new(key))?
            .downcast::<CFNumber>()?
            .to_f64()
    };
    let enabled = number("HTTPSEnable")?;
    if enabled != 1.0 {
        return None;
    }
    let host = settings
        .find(CFString::new("HTTPSProxy"))?
        .downcast::<CFString>()?
        .to_string();
    format_proxy(enabled, &host, number("HTTPSPort")?)
}

fn format_proxy(enabled: f64, host: &str, port: f64) -> Option<String> {
    if enabled != 1.0
        || host.is_empty()
        || host.len() > 253
        || !port.is_finite()
        || port.fract() != 0.0
        || !(1.0..=65535.0).contains(&port)
    {
        return None;
    }
    let authority = if let Ok(ipv6) = host.parse::<Ipv6Addr>() {
        format!("[{ipv6}]")
    } else if let Some(inner) = host.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        format!("[{}]", inner.parse::<Ipv6Addr>().ok()?)
    } else if host
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        host.to_owned()
    } else {
        // No userinfo, embedded port, path, percent escape, whitespace or URL
        // delimiters can turn a system host into a different proxy authority.
        return None;
    };
    // The HTTPS destination uses an HTTP CONNECT proxy, matching macOS's
    // manual HTTPS proxy setting rather than treating it as a TLS proxy URL.
    Some(format!("http://{authority}:{}", port as u16))
}

#[cfg(test)]
mod tests {
    use super::format_proxy;

    #[test]
    fn formats_dns_ipv4_and_ipv6_connect_proxies() {
        for (host, expected) in [
            ("proxy.example.com", "http://proxy.example.com:8080"),
            ("localhost", "http://localhost:8080"),
            ("127.0.0.1", "http://127.0.0.1:8080"),
            ("::1", "http://[::1]:8080"),
            ("[2001:db8::1]", "http://[2001:db8::1]:8080"),
        ] {
            assert_eq!(format_proxy(1.0, host, 8080.0).as_deref(), Some(expected));
        }
        assert_eq!(
            format_proxy(1.0, "proxy", 65535.0).as_deref(),
            Some("http://proxy:65535")
        );
    }

    #[test]
    fn rejects_disabled_fractional_and_out_of_range_settings() {
        for enabled in [0.0, -1.0, 0.5, 2.0, f64::NAN] {
            assert!(format_proxy(enabled, "proxy", 8080.0).is_none());
        }
        for port in [-1.0, 0.0, 80.5, 65536.0, f64::NAN, f64::INFINITY] {
            assert!(format_proxy(1.0, "proxy", port).is_none());
        }
    }

    #[test]
    fn rejects_host_authority_injection_and_malformed_ipv6() {
        for host in [
            "",
            " ",
            "proxy host",
            "proxy\n",
            "user@proxy",
            "proxy/path",
            "proxy?query",
            "proxy#fragment",
            "proxy:80",
            "proxy\\path",
            "%70roxy",
            "http://proxy",
            "[::1",
            "[invalid]",
            "::gg",
        ] {
            assert!(format_proxy(1.0, host, 8080.0).is_none(), "{host:?}");
        }
        assert!(format_proxy(1.0, &"a".repeat(254), 8080.0).is_none());
    }
}
