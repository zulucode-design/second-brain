use reqwest::{blocking::Client, redirect::Policy, StatusCode, Url};
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

const MAX_ARTICLE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClippedArticle {
    pub title: String,
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipError {
    InvalidUrl(String),
    Blocked(String),
    Fetch(String),
    Timeout(String),
    NotArticle(String),
}

impl ClipError {
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidUrl(message)
            | Self::Blocked(message)
            | Self::Fetch(message)
            | Self::Timeout(message)
            | Self::NotArticle(message) => message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    Timeout,
    Unreachable(String),
    Blocked(String),
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

fn validate_source_url(raw_url: &str) -> Result<reqwest::Url, ClipError> {
    let url = reqwest::Url::parse(raw_url)
        .map_err(|_| ClipError::InvalidUrl("Enter a valid web address".to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ClipError::InvalidUrl(
            "Only HTTP and HTTPS pages can be clipped".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ClipError::InvalidUrl(
            "Web addresses containing credentials cannot be clipped".to_string(),
        ));
    }
    let expected_port = if url.scheme() == "https" { 443 } else { 80 };
    if url.port_or_known_default() != Some(expected_port) {
        return Err(ClipError::Blocked(
            "Web addresses using nonstandard ports cannot be clipped".to_string(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ClipError::InvalidUrl("The web address has no host".to_string()))?;
    if host.ends_with('.') {
        return Err(ClipError::InvalidUrl(
            "The web address host must not end with a dot".to_string(),
        ));
    }
    let normalized = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if normalized == "localhost" || normalized.ends_with(".localhost") {
        return Err(ClipError::Blocked(
            "Local and private web addresses cannot be clipped".to_string(),
        ));
    }
    if normalized
        .parse::<IpAddr>()
        .is_ok_and(|address| !is_public_ip(address))
    {
        return Err(ClipError::Blocked(
            "Local and private web addresses cannot be clipped".to_string(),
        ));
    }
    Ok(url)
}

pub fn fetch_article_html(raw_url: &str) -> Result<String, ClipError> {
    fetch_with(&fetch_public_html, raw_url)
}

pub fn clip_url(raw_url: &str) -> Result<(ClippedArticle, String), ClipError> {
    let canonical_url = validate_source_url(raw_url)?.to_string();
    let html = fetch_article_html(&canonical_url)?;
    let article = extract_article(&html, &canonical_url)?;
    Ok((article, canonical_url))
}

fn fetch_with<Source>(source: &Source, raw_url: &str) -> Result<String, ClipError>
where
    Source: Fn(&reqwest::Url) -> Result<String, SourceError>,
{
    let url = validate_source_url(raw_url)?;
    source(&url).map_err(|error| match error {
        SourceError::Timeout => ClipError::Timeout(
            "The web page took too long to respond. Check your connection and try again."
                .to_string(),
        ),
        SourceError::Unreachable(detail) => ClipError::Fetch(detail),
        SourceError::Blocked(detail) => ClipError::Blocked(detail),
    })
}

fn resolve_public_host(url: &Url) -> Result<(String, Vec<SocketAddr>), SourceError> {
    let host = url
        .host_str()
        .ok_or_else(|| SourceError::Unreachable("The web address has no host".to_string()))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| SourceError::Unreachable("The web address has no port".to_string()))?;
    let addresses: Vec<_> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| {
            SourceError::Unreachable("The web page host could not be reached".to_string())
        })?
        .collect();
    if addresses.is_empty() {
        return Err(SourceError::Unreachable(
            "The web page host resolved to no addresses".to_string(),
        ));
    }
    if addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(SourceError::Blocked(
            "The web page host resolves to a local or private address".to_string(),
        ));
    }
    Ok((host, addresses))
}

fn html_client(host: &str, addresses: &[SocketAddr]) -> Result<Client, SourceError> {
    Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(Policy::none())
        .resolve_to_addrs(host, addresses)
        .build()
        .map_err(|_| SourceError::Unreachable("The web request could not be prepared".to_string()))
}

fn fetch_public_html(url: &Url) -> Result<String, SourceError> {
    let mut current_url = url.clone();
    for redirect_count in 0..=5 {
        let html = fetch_public_html_without_redirects(&current_url, redirect_count)?;
        match html {
            FetchStep::Html(html) => return Ok(html),
            FetchStep::Redirect(next_url) => current_url = next_url,
        }
    }
    Err(SourceError::Blocked(
        "The web page redirected too many times".to_string(),
    ))
}

enum FetchStep {
    Html(String),
    Redirect(Url),
}

fn fetch_public_html_without_redirects(
    url: &Url,
    redirect_count: usize,
) -> Result<FetchStep, SourceError> {
    let (host, addresses) = resolve_public_host(url)?;
    let client = html_client(&host, &addresses)?;

    let mut response = client.get(url.clone()).send().map_err(|error| {
        if error.is_timeout() {
            SourceError::Timeout
        } else {
            SourceError::Unreachable("The web page could not be reached".to_string())
        }
    })?;
    if response.status().is_redirection() {
        if redirect_count >= 5 {
            return Err(SourceError::Blocked(
                "The web page redirected too many times".to_string(),
            ));
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                SourceError::Unreachable(
                    "The web page redirected without a destination".to_string(),
                )
            })?;
        return Ok(FetchStep::Redirect(redirect_target(url, location)?));
    }
    if !response.status().is_success() {
        return Err(status_failure(response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARTICLE_BYTES)
    {
        return Err(SourceError::Blocked(
            "The web page exceeds the 5 MiB clipping limit".to_string(),
        ));
    }
    if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        let content_type = content_type
            .to_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !content_type.starts_with("text/html")
            && !content_type.starts_with("application/xhtml+xml")
        {
            return Err(SourceError::Blocked(
                "The web address did not return an HTML page".to_string(),
            ));
        }
    }

    let mut body = Vec::new();
    response
        .by_ref()
        .take(MAX_ARTICLE_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|_| SourceError::Unreachable("The web page could not be read".to_string()))?;
    if body.len() as u64 > MAX_ARTICLE_BYTES {
        return Err(SourceError::Blocked(
            "The web page exceeds the 5 MiB clipping limit".to_string(),
        ));
    }
    let html = String::from_utf8(body)
        .map_err(|_| SourceError::Blocked("The web page is not valid UTF-8 HTML".to_string()))?;
    Ok(FetchStep::Html(html))
}

fn redirect_target(current_url: &Url, location: &str) -> Result<Url, SourceError> {
    let next_url = current_url.join(location).map_err(|_| {
        SourceError::Unreachable("The web page redirected to an invalid URL".to_string())
    })?;
    validate_source_url(next_url.as_str()).map_err(|error| match error {
        ClipError::InvalidUrl(message) | ClipError::Blocked(message) => {
            SourceError::Blocked(message)
        }
        ClipError::Fetch(message)
        | ClipError::Timeout(message)
        | ClipError::NotArticle(message) => SourceError::Unreachable(message),
    })
}

fn status_failure(status: StatusCode) -> SourceError {
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return SourceError::Blocked(
            "The web page requires login or permission before it can be clipped".to_string(),
        );
    }
    SourceError::Unreachable(format!("The web page returned HTTP {status}"))
}

pub fn extract_article(html: &str, source_url: &str) -> Result<ClippedArticle, ClipError> {
    if looks_like_login_wall(html) {
        return Err(ClipError::NotArticle(
            "The web page requires login before it can be clipped".to_string(),
        ));
    }
    let article = legible::parse(html, Some(source_url), None)
        .map_err(|error| ClipError::NotArticle(error.to_string()))?;
    if article.text_content.split_whitespace().count() < 20 {
        return Err(ClipError::NotArticle(
            "No readable article content was found on that page".to_string(),
        ));
    }
    Ok(ClippedArticle {
        title: article.title.trim().to_string(),
        markdown: article.markdown_content.trim().to_string(),
    })
}

fn looks_like_login_wall(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("<form")
        && (lower.contains("type=\"password\"")
            || lower.contains("type='password'")
            || lower.contains(">sign in<")
            || lower.contains(">log in<")
            || lower.contains("login"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_article_as_markdown_without_page_furniture() {
        let html = r#"
            <!doctype html>
            <html>
              <head><title>Field Notes | Example</title></head>
              <body>
                <nav>Home Products Pricing</nav>
                <aside class="advertisement">Buy our unrelated product</aside>
                <main>
                  <article>
                    <h1>Field Notes</h1>
                    <p>Careful observation turns scattered details into useful knowledge.</p>
                    <p>Writing those observations down makes patterns visible over time and
                       gives future work a reliable point of departure.</p>
                  </article>
                </main>
                <script>stealSecrets()</script>
              </body>
            </html>
        "#;

        let article = extract_article(html, "https://example.com/field-notes")
            .expect("the page contains a readable article");

        assert_eq!(article.title, "Field Notes | Example");
        assert!(article.markdown.contains("Careful observation"));
        assert!(article.markdown.contains("patterns visible over time"));
        assert!(!article.markdown.contains("Products Pricing"));
        assert!(!article.markdown.contains("unrelated product"));
        assert!(!article.markdown.contains("stealSecrets"));
    }

    #[test]
    fn rejects_a_login_wall_instead_of_creating_an_empty_clipping() {
        let html = r#"
            <html>
              <head><title>Sign in</title></head>
              <body>
                <main>
                  <h1>Sign in to continue</h1>
                  <form><input type="password"><button>Sign in</button></form>
                </main>
              </body>
            </html>
        "#;

        let error = extract_article(html, "https://example.com/login")
            .expect_err("a login wall is not an article");

        assert!(matches!(error, ClipError::NotArticle(_)));
        assert!(error.message().contains("requires login"));
    }

    #[test]
    fn rejects_non_article_content_without_calling_it_a_login_page() {
        let html = r#"
            <html>
              <head><title>Product chooser</title></head>
              <body><main><h1>Pick a plan</h1><p>Short page.</p></main></body>
            </html>
        "#;

        let error = extract_article(html, "https://example.com/plans")
            .expect_err("thin marketing furniture is not an article");

        assert!(matches!(error, ClipError::NotArticle(_)));
        assert!(error.message().contains("No readable article content"));
        assert!(!error.message().contains("login"));
    }

    #[test]
    fn rejects_non_web_and_private_sources_before_fetching() {
        for url in [
            "file:///etc/passwd",
            "http://localhost/article",
            "http://127.0.0.1/article",
            "http://[::1]/article",
            "https://user:secret@example.com/article",
            "https://example.com:8443/article",
        ] {
            assert!(
                matches!(
                    fetch_article_html(url),
                    Err(ClipError::InvalidUrl(_) | ClipError::Blocked(_))
                ),
                "accepted {url}"
            );
        }
    }

    #[test]
    fn reports_a_timeout_as_a_distinct_actionable_failure() {
        let source = |_url: &reqwest::Url| Err(SourceError::Timeout);

        let error =
            fetch_with(&source, "https://example.com/article").expect_err("the source timed out");

        assert!(matches!(error, ClipError::Timeout(_)));
    }

    #[test]
    fn reports_login_required_statuses_distinctly() {
        for status in [
            reqwest::StatusCode::UNAUTHORIZED,
            reqwest::StatusCode::FORBIDDEN,
        ] {
            let error = status_failure(status);

            assert!(
                matches!(error, SourceError::Blocked(message) if message.contains("requires login"))
            );
        }
    }

    #[test]
    fn accepts_safe_cross_host_redirect_targets_for_public_web_pages() {
        let current = Url::parse("https://example.com/story").unwrap();
        let next = redirect_target(&current, "https://www.example.com/story").unwrap();

        assert_eq!(next.as_str(), "https://www.example.com/story");
    }

    #[test]
    fn rejects_redirect_targets_to_private_addresses() {
        let current = Url::parse("https://example.com/story").unwrap();
        let error = redirect_target(&current, "http://127.0.0.1/story")
            .expect_err("private redirects stay blocked");

        assert!(matches!(error, SourceError::Blocked(_)));
    }
}
