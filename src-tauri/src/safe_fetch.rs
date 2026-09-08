//! Fetching from arbitrary public web addresses, safely.
//!
//! Two features reach the open internet: web clipping and the image proxy. Both take a URL
//! the app did not choose, so both must refuse to become a probe of the machine's own
//! network. The private-range checks, the DNS pinning that stops a public name resolving to
//! a private address in the gap between validation and request, and the byte caps all exist
//! for that. One copy means a gap closed here is closed for both.
//!
//! What legitimately differs stays with the caller: redirect policy, accepted content types,
//! size limits, timeouts, and the wording of every refusal. This module reports *what* it
//! rejected and lets the caller say it in that feature's own voice.

use reqwest::blocking::{ClientBuilder, Response};
use reqwest::Url;
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

/// Sites that publish a User-Agent policy — Wikimedia among them — answer an unidentified
/// client with 403, which a caller can only report as a page needing permission. Naming the
/// app and where it comes from is what those policies ask for.
const USER_AGENT: &str = concat!(
    "SecondBrain/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/zulucode-design/second-brain)"
);

/// Why an address is not something the app will fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlRejection {
    Unparseable,
    NotHttp,
    HasCredentials,
    NonStandardPort,
    NoHost,
    TrailingDot,
    /// A name that resolves inside this machine, such as `localhost`.
    LocalName,
    /// A literal address in a private, loopback, link-local, or reserved range.
    PrivateAddress,
}

/// Why a host cannot be turned into an address worth connecting to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRejection {
    NoHost,
    NoPort,
    Unresolvable(String),
    NoAddresses,
    PrivateAddress,
}

/// A body that was read, or refused for exceeding the caller's cap.
#[derive(Debug)]
pub enum Body {
    Complete(Vec<u8>),
    TooLarge,
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 0 && c == 2)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            if ip.is_unspecified() || ip.is_loopback() || ip.to_ipv4().is_some() {
                return false;
            }
            let first = ip.segments()[0];
            (0x2000..=0x3fff).contains(&first) && ip.segments()[..2] != [0x2001, 0x0db8]
        }
    }
}

/// The host as it should be compared and resolved: no brackets, no trailing dot, lowercase.
pub fn normalized_host(host: &str) -> String {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// Everything that can be decided about an address without touching the network.
///
/// A nonstandard port is refused rather than merely noted: it is the shape a request to a
/// local service takes, and no article or image the app should be fetching needs one.
pub fn validate_public_url(raw_url: &str) -> Result<Url, UrlRejection> {
    let url = Url::parse(raw_url).map_err(|_| UrlRejection::Unparseable)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(UrlRejection::NotHttp);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UrlRejection::HasCredentials);
    }
    let expected_port = if url.scheme() == "https" { 443 } else { 80 };
    if url.port_or_known_default() != Some(expected_port) {
        return Err(UrlRejection::NonStandardPort);
    }
    let host = url.host_str().ok_or(UrlRejection::NoHost)?;
    if host.ends_with('.') {
        return Err(UrlRejection::TrailingDot);
    }
    let normalized = normalized_host(host);
    if normalized == "localhost" || normalized.ends_with(".localhost") {
        return Err(UrlRejection::LocalName);
    }
    if normalized
        .parse::<IpAddr>()
        .is_ok_and(|address| !is_public_ip(address))
    {
        return Err(UrlRejection::PrivateAddress);
    }
    Ok(url)
}

/// Resolve a host and keep the addresses, so the request connects to what was checked.
///
/// Returning the addresses is the point. Validating a name and then letting the client
/// resolve it again leaves a window where the second answer differs from the first; pinning
/// the result closes it.
pub fn resolve_public_addrs(url: &Url) -> Result<(String, Vec<SocketAddr>), HostRejection> {
    let host = normalized_host(url.host_str().ok_or(HostRejection::NoHost)?);
    let port = url.port_or_known_default().ok_or(HostRejection::NoPort)?;
    let addresses: Vec<_> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| HostRejection::Unresolvable(error.to_string()))?
        .collect();
    if addresses.is_empty() {
        return Err(HostRejection::NoAddresses);
    }
    if addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(HostRejection::PrivateAddress);
    }
    Ok((host, addresses))
}

/// A client identified to the sites it calls and pinned to already-checked addresses.
///
/// Returned as a builder because redirect policy is the one thing the two callers cannot
/// share: the image proxy refuses to leave its host, while clipping must follow the
/// apex-to-www hops that ordinary articles rely on.
pub fn pinned_client(host: &str, addresses: &[SocketAddr], timeout: Duration) -> ClientBuilder {
    ClientBuilder::new()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .resolve_to_addrs(host, addresses)
}

/// Read a body, refusing anything past the cap.
///
/// The declared length is only a hint, so it is checked first as a courtesy and then
/// enforced by reading one byte more than allowed: a server that lies, or sends no length
/// at all, still cannot make the app hold an unbounded buffer.
pub fn read_capped(response: &mut Response, max_bytes: u64) -> Result<Body, std::io::Error> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes)
    {
        return Ok(Body::TooLarge);
    }
    let mut body = Vec::new();
    response
        .by_ref()
        .take(max_bytes + 1)
        .read_to_end(&mut body)?;
    if body.len() as u64 > max_bytes {
        return Ok(Body::TooLarge);
    }
    Ok(Body::Complete(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn treats_every_private_range_as_off_limits() {
        for ip in [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            "fc00::1".parse().unwrap(),
            "fe80::1".parse().unwrap(),
        ] {
            assert!(!is_public_ip(ip), "{ip}");
        }
        assert!(is_public_ip("93.184.216.34".parse().unwrap()));
        assert!(is_public_ip(
            "2606:2800:220:1:248:1893:25c8:1946".parse().unwrap()
        ));
    }

    #[test]
    fn names_the_reason_each_address_is_refused() {
        for (url, expected) in [
            ("not a url", UrlRejection::Unparseable),
            ("file:///etc/passwd", UrlRejection::NotHttp),
            (
                "https://user:pass@example.com/a",
                UrlRejection::HasCredentials,
            ),
            ("http://example.com:8080/a", UrlRejection::NonStandardPort),
            ("https://example.com./a", UrlRejection::TrailingDot),
            ("http://localhost/a", UrlRejection::LocalName),
            ("http://127.0.0.1/a", UrlRejection::PrivateAddress),
            ("http://[::1]/a", UrlRejection::PrivateAddress),
        ] {
            assert_eq!(validate_public_url(url), Err(expected), "{url}");
        }
        assert!(validate_public_url("https://example.com/a").is_ok());
        assert!(validate_public_url("http://93.184.216.34/a").is_ok());
    }

    /// The caller cannot distinguish a refusal it must explain from one it must not, unless
    /// the local-name and private-address cases stay separate.
    #[test]
    fn separates_a_local_name_from_a_private_literal() {
        assert_eq!(
            validate_public_url("http://sub.localhost/a"),
            Err(UrlRejection::LocalName)
        );
        assert_eq!(
            validate_public_url("http://10.0.0.1/a"),
            Err(UrlRejection::PrivateAddress)
        );
    }
}
