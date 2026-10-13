//! The proxy the system would use for an upstream URL, for accounts without a
//! proxy of their own: the process environment (`HTTPS_PROXY`, `HTTP_PROXY`,
//! `ALL_PROXY`, `NO_PROXY`), then the system settings — on macOS through
//! CFNetwork, PAC included; on Linux GNOME's manual proxy; on Windows the
//! Internet Settings' fixed proxy. Loopback hosts are always reached directly.
use reqwest::Url;
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

/// System settings, PAC files included, are read again after this long.
const TTL: Duration = Duration::from_secs(30);

pub(crate) fn for_url(url: &Url) -> Option<Url> {
    let host = url.host_str()?;
    if loopback(host) {
        return None;
    }
    if let Some(found) = from_env(url) {
        return found;
    }
    // Resolving can fetch a PAC file or run a subprocess, and this is called
    // from reqwest's connector on the async runtime: an expired entry answers
    // while a thread renews it, and only a host's first lookup waits.
    struct Entry {
        at: Instant,
        found: Option<Url>,
        renewing: bool,
    }
    static CACHE: LazyLock<Mutex<HashMap<String, Entry>>> = LazyLock::new(Default::default);
    let key = format!("{}://{host}:{}", url.scheme(), url.port_or_known_default()?);
    let store = |key: String, found: Option<Url>| {
        if let Ok(mut cache) = CACHE.lock() {
            cache.insert(
                key,
                Entry {
                    at: Instant::now(),
                    found,
                    renewing: false,
                },
            );
        }
    };
    if let Some(entry) = CACHE.lock().ok()?.get_mut(&key) {
        if entry.at.elapsed() >= TTL && !entry.renewing {
            entry.renewing = true;
            let (key, url) = (key.clone(), url.clone());
            std::thread::spawn(move || store(key, platform::resolve(&url)));
        }
        return entry.found.clone();
    }
    let found = off_runtime(|| platform::resolve(url));
    store(key, found.clone());
    found
}

/// Runs blocking work without stalling the other tasks of a multi-threaded
/// runtime's worker.
fn off_runtime<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(work)
        }
        _ => work(),
    }
}

fn loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// `Some(None)`: an environment proxy is configured but `NO_PROXY` exempts
/// the host; `None`: the environment configures no proxy for this scheme.
fn from_env(url: &Url) -> Option<Option<Url>> {
    let var = |name: &str| {
        std::env::var(name)
            .or_else(|_| std::env::var(name.to_lowercase()))
            .ok()
            .filter(|v| !v.trim().is_empty())
    };
    let value = if url.scheme() == "https" {
        var("HTTPS_PROXY")
    } else {
        var("HTTP_PROXY")
    }
    .or_else(|| var("ALL_PROXY"))?;
    if exempt(
        url.host_str()?,
        var("NO_PROXY").unwrap_or_default().split(','),
    ) {
        return Some(None);
    }
    Some(proxy_url(&value))
}

/// `NO_PROXY`-style exemptions: `*`, exact hosts, and domain suffixes with or
/// without a leading dot.
fn exempt<'a>(host: &str, rules: impl IntoIterator<Item = &'a str>) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_lowercase();
    rules.into_iter().any(|rule| {
        let rule = rule.trim().trim_start_matches("*.").to_lowercase();
        let rule = rule.trim_start_matches('.');
        rule == "*" || (!rule.is_empty() && (host == rule || host.ends_with(&format!(".{rule}"))))
    })
}

/// `host:port` or a URL; SOCKS resolves names through the proxy.
fn proxy_url(value: &str) -> Option<Url> {
    let value = value.trim();
    let mut url = Url::parse(&if value.contains("://") {
        value.to_owned()
    } else {
        format!("http://{value}")
    })
    .ok()?;
    if matches!(url.scheme(), "socks5" | "socks") {
        url.set_scheme("socks5h").ok()?;
    }
    url.host_str()?;
    Some(url)
}

#[cfg(target_os = "macos")]
mod platform {
    //! CFNetwork's own answer for a URL: fixed proxies, exceptions and PAC.
    use super::{proxy_url, Url, TTL};
    use std::{
        collections::HashMap,
        ffi::{c_char, c_void, CString},
        sync::{LazyLock, Mutex},
        time::Instant,
    };
    type CFTypeRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const SINT64: isize = 4;
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
        fn CFStringCreateWithCString(
            alloc: CFTypeRef,
            s: *const c_char,
            encoding: u32,
        ) -> CFTypeRef;
        fn CFStringGetCString(s: CFTypeRef, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFStringGetLength(s: CFTypeRef) -> isize;
        fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
        fn CFURLCreateWithString(alloc: CFTypeRef, s: CFTypeRef, base: CFTypeRef) -> CFTypeRef;
        fn CFArrayGetCount(array: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
        fn CFDictionaryGetValue(dictionary: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFNumberGetValue(number: CFTypeRef, kind: isize, value: *mut c_void) -> u8;
    }
    #[link(name = "CFNetwork", kind = "framework")]
    extern "C" {
        static kCFProxyTypeKey: CFTypeRef;
        static kCFProxyTypeNone: CFTypeRef;
        static kCFProxyTypeHTTP: CFTypeRef;
        static kCFProxyTypeHTTPS: CFTypeRef;
        static kCFProxyTypeSOCKS: CFTypeRef;
        static kCFProxyTypeAutoConfigurationURL: CFTypeRef;
        static kCFProxyTypeAutoConfigurationJavaScript: CFTypeRef;
        static kCFProxyAutoConfigurationURLKey: CFTypeRef;
        static kCFProxyAutoConfigurationJavaScriptKey: CFTypeRef;
        static kCFProxyHostNameKey: CFTypeRef;
        static kCFProxyPortNumberKey: CFTypeRef;
        fn CFNetworkCopySystemProxySettings() -> CFTypeRef;
        fn CFNetworkCopyProxiesForURL(url: CFTypeRef, settings: CFTypeRef) -> CFTypeRef;
        fn CFNetworkCopyProxiesForAutoConfigurationScript(
            script: CFTypeRef,
            url: CFTypeRef,
            error: *mut CFTypeRef,
        ) -> CFTypeRef;
    }
    /// Releases a Create/Copy result when dropped.
    struct Owned(CFTypeRef);
    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) }
            }
        }
    }
    fn cf_string(value: &str) -> Option<Owned> {
        let value = CString::new(value).ok()?;
        let string = unsafe { CFStringCreateWithCString(std::ptr::null(), value.as_ptr(), UTF8) };
        (!string.is_null()).then_some(Owned(string))
    }
    unsafe fn rust_string(value: CFTypeRef) -> Option<String> {
        if value.is_null() {
            return None;
        }
        // Inline PAC scripts run to tens of kilobytes.
        let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(value), UTF8);
        let mut buffer = vec![0 as c_char; usize::try_from(size).ok()?.checked_add(1)?];
        (CFStringGetCString(value, buffer.as_mut_ptr(), buffer.len() as isize, UTF8) != 0)
            .then(|| unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) })
            .map(|s| s.to_string_lossy().into_owned())
    }
    pub(super) fn resolve(url: &Url) -> Option<Url> {
        unsafe {
            let settings = Owned(CFNetworkCopySystemProxySettings());
            let address = cf_string(url.as_str())?;
            let target = Owned(CFURLCreateWithString(
                std::ptr::null(),
                address.0,
                std::ptr::null(),
            ));
            if settings.0.is_null() || target.0.is_null() {
                return None;
            }
            let proxies = Owned(CFNetworkCopyProxiesForURL(target.0, settings.0));
            choose(proxies.0, target.0, 0).flatten()
        }
    }
    /// The first usable entry, in CFNetwork's order of preference: `Some(None)`
    /// for a direct connection, `None` when no entry could be used.
    unsafe fn choose(proxies: CFTypeRef, target: CFTypeRef, depth: u8) -> Option<Option<Url>> {
        if proxies.is_null() {
            return None;
        }
        for index in 0..CFArrayGetCount(proxies) {
            let proxy = CFArrayGetValueAtIndex(proxies, index);
            let kind = CFDictionaryGetValue(proxy, kCFProxyTypeKey);
            let is = |other: CFTypeRef| !kind.is_null() && CFEqual(kind, other) != 0;
            if is(kCFProxyTypeNone) {
                return Some(None);
            }
            let scheme = if is(kCFProxyTypeHTTP) || is(kCFProxyTypeHTTPS) {
                "http"
            } else if is(kCFProxyTypeSOCKS) {
                "socks5h"
            } else {
                // A PAC file, by URL or inline; its own entries come first.
                if depth > 0 {
                    continue;
                }
                let script = if is(kCFProxyTypeAutoConfigurationURL) {
                    let address = CFDictionaryGetValue(proxy, kCFProxyAutoConfigurationURLKey);
                    // The key holds a CFURL; its description is the address.
                    rust_string(CFURLGetString(address)).and_then(|a| pac_script(&a))
                } else if is(kCFProxyTypeAutoConfigurationJavaScript) {
                    rust_string(CFDictionaryGetValue(
                        proxy,
                        kCFProxyAutoConfigurationJavaScriptKey,
                    ))
                } else {
                    None
                };
                let Some(script) = script.as_deref().and_then(cf_string) else {
                    continue;
                };
                let mut error = std::ptr::null();
                let evaluated = Owned(CFNetworkCopyProxiesForAutoConfigurationScript(
                    script.0, target, &mut error,
                ));
                if !error.is_null() {
                    CFRelease(error);
                }
                match choose(evaluated.0, target, depth + 1) {
                    Some(found) => return Some(found),
                    None => continue,
                }
            };
            let host = rust_string(CFDictionaryGetValue(proxy, kCFProxyHostNameKey));
            let mut port = 0i64;
            let number = CFDictionaryGetValue(proxy, kCFProxyPortNumberKey);
            if !number.is_null() {
                CFNumberGetValue(number, SINT64, (&mut port as *mut i64).cast());
            }
            if let Some(found) = host
                .filter(|_| port > 0)
                .and_then(|host| proxy_url(&format!("{scheme}://{}:{port}", bracket(&host))))
            {
                return Some(Some(found));
            }
        }
        None
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFURLGetString(url: CFTypeRef) -> CFTypeRef;
    }
    fn bracket(host: &str) -> String {
        if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_owned()
        }
    }
    /// The PAC file at `address`, cached like resolved proxies.
    fn pac_script(address: &str) -> Option<String> {
        static CACHE: LazyLock<Mutex<HashMap<String, (Instant, Option<String>)>>> =
            LazyLock::new(Default::default);
        if let Some((at, script)) = CACHE.lock().ok()?.get(address) {
            if at.elapsed() < TTL {
                return script.clone();
            }
        }
        let script = fetch(address);
        CACHE
            .lock()
            .ok()?
            .insert(address.to_owned(), (Instant::now(), script.clone()));
        script
    }
    /// PAC files are local files or plain HTTP, usually served on loopback by
    /// the proxy client; HTTP/1.0 keeps the response unchunked.
    fn fetch(address: &str) -> Option<String> {
        use std::io::{Read, Write};
        let url = Url::parse(address).ok()?;
        if url.scheme() == "file" {
            return std::fs::read_to_string(url.to_file_path().ok()?).ok();
        }
        if url.scheme() != "http" {
            return None;
        }
        let timeout = std::time::Duration::from_secs(2);
        let socket = std::net::ToSocketAddrs::to_socket_addrs(&(
            url.host_str()?,
            url.port_or_known_default()?,
        ))
        .ok()?
        .next()?;
        let mut stream = std::net::TcpStream::connect_timeout(&socket, timeout).ok()?;
        stream.set_read_timeout(Some(timeout)).ok()?;
        let path = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        };
        write!(
            stream,
            "GET {path} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
            url.host_str()?
        )
        .ok()?;
        let mut response = Vec::new();
        stream.take(1 << 20).read_to_end(&mut response).ok()?;
        let response = String::from_utf8_lossy(&response);
        let (head, body) = response.split_once("\r\n\r\n")?;
        head.split_whitespace()
            .nth(1)
            .filter(|status| *status == "200")
            .map(|_| body.to_owned())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    //! GNOME's manual proxy (`gsettings`); its automatic (PAC) mode is not
    //! evaluated, and connects directly.
    use super::{exempt, proxy_url, Url};
    fn get(schema: &str, key: &str) -> Option<String> {
        let output = std::process::Command::new("gsettings")
            .args(["get", schema, key])
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
    fn unquote(value: &str) -> &str {
        value.trim().trim_matches('\'')
    }
    pub(super) fn resolve(url: &Url) -> Option<Url> {
        if unquote(&get("org.gnome.system.proxy", "mode")?) != "manual" {
            return None;
        }
        let ignored = get("org.gnome.system.proxy", "ignore-hosts").unwrap_or_default();
        let ignored = ignored.trim_matches(|c| c == '[' || c == ']');
        if exempt(url.host_str()?, ignored.split(',').map(unquote)) {
            return None;
        }
        let schema = if url.scheme() == "https" {
            "https"
        } else {
            "http"
        };
        [(schema, "http"), ("socks", "socks5h")]
            .into_iter()
            .find_map(|(schema, scheme)| {
                let schema = format!("org.gnome.system.proxy.{schema}");
                let host = get(&schema, "host")?;
                let host = unquote(&host);
                let port: u16 = get(&schema, "port")?.parse().ok()?;
                (!host.is_empty() && port > 0)
                    .then(|| proxy_url(&format!("{scheme}://{host}:{port}")))
                    .flatten()
            })
    }
}

#[cfg(target_os = "windows")]
mod platform {
    //! The Internet Settings' fixed proxy; an automatic configuration script
    //! is not evaluated, and connects directly.
    use super::{proxy_url, Url};
    pub(super) fn resolve(url: &Url) -> Option<Url> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        super::windows_proxy(&String::from_utf8_lossy(&output.stdout), url)
            .and_then(|value| proxy_url(&value))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod platform {
    pub(super) fn resolve(_: &super::Url) -> Option<super::Url> {
        None
    }
}

/// The proxy in `reg query` output of Internet Settings: `ProxyServer` is
/// `host:port` or `http=…;https=…;socks=…`, `ProxyOverride` lists exempt
/// hosts (`<local>` for names without a dot).
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn windows_proxy(registry: &str, url: &Url) -> Option<String> {
    let value = |name: &str| {
        registry.lines().find_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next()? == name).then(|| fields.nth(1).unwrap_or_default().to_owned())
        })
    };
    if value("ProxyEnable")? != "0x1" {
        return None;
    }
    let host = url.host_str()?;
    let overrides = value("ProxyOverride").unwrap_or_default();
    let local = !host.contains('.');
    if overrides.split(';').any(|rule| {
        let rule = rule.trim();
        (rule == "<local>" && local) || exempt(host, [rule])
    }) {
        return None;
    }
    let server = value("ProxyServer")?;
    if !server.contains('=') {
        return Some(server);
    }
    let entry = |kind: &str| {
        server.split(';').find_map(|part| {
            let (name, address) = part.split_once('=')?;
            name.trim()
                .eq_ignore_ascii_case(kind)
                .then(|| address.trim().to_owned())
        })
    };
    entry(if url.scheme() == "https" {
        "https"
    } else {
        "http"
    })
    .map(|address| format!("http://{address}"))
    .or_else(|| entry("socks").map(|address| format!("socks5h://{address}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exemptions_match_hosts_and_domain_suffixes() {
        assert!(exempt("api.example.com", ["example.com"]));
        assert!(exempt("api.example.com", [".example.com"]));
        assert!(exempt("api.example.com", ["*.example.com"]));
        assert!(exempt("anything", ["*"]));
        assert!(!exempt("example.community", ["example.com"]));
        assert!(!exempt("example.com", [""]));
        assert!(loopback("127.0.0.1") && loopback("[::1]") && loopback("LOCALHOST"));
        assert!(!loopback("chatgpt.com"));
    }
    #[test]
    fn proxies_are_urls_and_socks_resolves_through_the_proxy() {
        assert_eq!(
            proxy_url("127.0.0.1:7890").unwrap().as_str(),
            "http://127.0.0.1:7890/"
        );
        assert_eq!(
            proxy_url("socks5://127.0.0.1:1080").unwrap().as_str(),
            "socks5h://127.0.0.1:1080"
        );
    }
    #[test]
    fn windows_internet_settings_choose_per_scheme_and_honour_overrides() {
        let url = |u: &str| Url::parse(u).unwrap();
        let registry = "    ProxyEnable    REG_DWORD    0x1\n    ProxyServer    REG_SZ    http=10.0.0.1:8080;https=10.0.0.2:8443;socks=10.0.0.3:1080\n    ProxyOverride    REG_SZ    <local>;*.corp.example\n";
        assert_eq!(
            windows_proxy(registry, &url("https://chatgpt.com/")).as_deref(),
            Some("http://10.0.0.2:8443")
        );
        assert_eq!(
            windows_proxy(registry, &url("http://chatgpt.com/")).as_deref(),
            Some("http://10.0.0.1:8080")
        );
        assert_eq!(windows_proxy(registry, &url("https://intranet/")), None);
        assert_eq!(
            windows_proxy(registry, &url("https://a.corp.example/")),
            None
        );
        let single =
            "    ProxyEnable    REG_DWORD    0x1\n    ProxyServer    REG_SZ    10.0.0.9:3128\n";
        assert_eq!(
            windows_proxy(single, &url("https://chatgpt.com/")).as_deref(),
            Some("10.0.0.9:3128")
        );
        let disabled =
            "    ProxyEnable    REG_DWORD    0x0\n    ProxyServer    REG_SZ    10.0.0.9:3128\n";
        assert_eq!(windows_proxy(disabled, &url("https://chatgpt.com/")), None);
    }
}
