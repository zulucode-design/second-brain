use crate::safe_fetch::{self, Body, HostRejection, UrlRejection};
use reqwest::{blocking::Client, redirect::Policy, Url};
use std::net::SocketAddr;
use std::time::Duration;

const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

pub struct ImageResponse {
    pub content_type: String,
    pub body: Vec<u8>,
}

pub enum ProxyError {
    Invalid(String),
    Blocked(String),
    Fetch(String),
}

impl ProxyError {
    pub fn status(&self) -> u16 {
        match self {
            Self::Invalid(_) => 400,
            Self::Blocked(_) => 403,
            Self::Fetch(_) => 502,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(message) | Self::Blocked(message) | Self::Fetch(message) => message,
        }
    }
}

/// The proxy's wording for an address it will not fetch.
fn validate_url(raw_url: &str) -> Result<Url, ProxyError> {
    safe_fetch::validate_public_url(raw_url).map_err(|rejection| match rejection {
        UrlRejection::Unparseable => ProxyError::Invalid("Invalid image URL".into()),
        UrlRejection::NotHttp => {
            ProxyError::Invalid("Only HTTP and HTTPS images are supported".into())
        }
        UrlRejection::HasCredentials => {
            ProxyError::Invalid("Image URLs must not contain credentials".into())
        }
        UrlRejection::NonStandardPort => ProxyError::Blocked("Non-standard image URL port".into()),
        UrlRejection::NoHost => ProxyError::Invalid("Image URL has no host".into()),
        UrlRejection::TrailingDot => {
            ProxyError::Invalid("Image URL host must not have a trailing dot".into())
        }
        UrlRejection::LocalName => ProxyError::Blocked("Local image hosts are not allowed".into()),
        UrlRejection::PrivateAddress => {
            ProxyError::Blocked("Private image addresses are not allowed".into())
        }
    })
}

fn resolve_public_host(url: &Url) -> Result<(String, Vec<SocketAddr>), ProxyError> {
    safe_fetch::resolve_public_addrs(url).map_err(|rejection| match rejection {
        HostRejection::NoHost => ProxyError::Invalid("Image URL has no host".into()),
        HostRejection::NoPort => ProxyError::Invalid("Image URL has no port".into()),
        HostRejection::Unresolvable(error) => {
            ProxyError::Fetch(format!("Could not resolve image host: {error}"))
        }
        HostRejection::NoAddresses => {
            ProxyError::Fetch("Image host resolved to no addresses".into())
        }
        HostRejection::PrivateAddress => {
            ProxyError::Blocked("Image host resolves to a private address".into())
        }
    })
}

/// Redirects may be followed automatically here, unlike in clipping, because they may not
/// leave the host the addresses were pinned to — so no hop can reach an address that was
/// never checked.
fn image_client(host: &str, addresses: &[SocketAddr]) -> Result<Client, ProxyError> {
    let redirect_host = host.to_string();
    safe_fetch::pinned_client(host, addresses, Duration::from_secs(30))
        .redirect(Policy::custom(move |attempt| {
            let redirected_elsewhere = match attempt.url().host_str() {
                Some(host) => safe_fetch::normalized_host(host) != redirect_host,
                None => true,
            };
            if attempt.previous().len() >= 5
                || redirected_elsewhere
                || validate_url(attempt.url().as_str()).is_err()
            {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|error| ProxyError::Fetch(format!("Could not create image client: {error}")))
}

pub fn fetch(raw_url: &str) -> Result<ImageResponse, ProxyError> {
    let url = validate_url(raw_url)?;
    let (host, addresses) = resolve_public_host(&url)?;
    let client = image_client(&host, &addresses)?;
    let mut response = client
        .get(url)
        .send()
        .map_err(|error| ProxyError::Fetch(format!("Image request failed: {error}")))?;
    if !response.status().is_success() {
        return Err(ProxyError::Fetch(format!(
            "Image server returned {}",
            response.status()
        )));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !content_type.to_ascii_lowercase().starts_with("image/") {
        return Err(ProxyError::Blocked(
            "Remote resource is not an image".into(),
        ));
    }
    let body = safe_fetch::read_capped(&mut response, MAX_IMAGE_BYTES)
        .map_err(|error| ProxyError::Fetch(format!("Could not read image: {error}")))?;
    let body = match body {
        Body::Complete(body) => body,
        Body::TooLarge => return Err(ProxyError::Blocked("Image exceeds the 20 MiB limit".into())),
    };
    Ok(ImageResponse { content_type, body })
}

#[cfg(test)]
mod tests {
    use super::validate_url;

    /// The private-range rules themselves are `safe_fetch`'s to test. What matters here is
    /// that the proxy still refuses each shape, with its own wording rather than the
    /// clipper's.
    #[test]
    fn validates_only_public_web_image_urls() {
        for url in [
            "file:///etc/passwd",
            "http://localhost/image.png",
            "http://127.0.0.1/image.png",
            "http://[::1]/image.png",
            "http://example.com:8080/image.png",
            "https://example.com./image.png",
            "https://user:pass@example.com/image.png",
        ] {
            assert!(validate_url(url).is_err(), "{url}");
        }
        assert!(validate_url("https://example.com/image.png").is_ok());
        assert!(validate_url("http://93.184.216.34/image.png").is_ok());
    }

    #[test]
    fn keeps_image_wording_rather_than_the_clipper_s() {
        assert_eq!(
            validate_url("http://localhost/image.png")
                .err()
                .map(|error| error.message().to_string()),
            Some("Local image hosts are not allowed".to_string())
        );
    }
}
