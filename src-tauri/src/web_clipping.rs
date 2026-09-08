use crate::safe_fetch::{self, Body, HostRejection, UrlRejection};
use reqwest::{blocking::Client, redirect::Policy, StatusCode, Url};
use std::net::SocketAddr;
use std::time::Duration;

const MAX_ARTICLE_BYTES: u64 = 5 * 1024 * 1024;

/// Below this, what the parser returned is a redirect stub or leftover page furniture rather
/// than something worth filing as a note.
///
/// It is deliberately not a paywall test. A soft paywall serves a teaser of real prose and
/// clears any threshold low enough to be safe, so raising this to catch one would start
/// refusing short articles instead.
const MIN_ARTICLE_WORDS: usize = 20;

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

/// The clipper's wording for an address it will not fetch.
///
/// Local and private addresses collapse to one message deliberately: distinguishing them
/// would tell whoever supplied the URL which internal names resolve.
fn validate_source_url(raw_url: &str) -> Result<Url, ClipError> {
    safe_fetch::validate_public_url(raw_url).map_err(|rejection| match rejection {
        UrlRejection::Unparseable => ClipError::InvalidUrl("Enter a valid web address".to_string()),
        UrlRejection::NotHttp => {
            ClipError::InvalidUrl("Only HTTP and HTTPS pages can be clipped".to_string())
        }
        UrlRejection::HasCredentials => ClipError::InvalidUrl(
            "Web addresses containing credentials cannot be clipped".to_string(),
        ),
        UrlRejection::NonStandardPort => ClipError::Blocked(
            "Web addresses using nonstandard ports cannot be clipped".to_string(),
        ),
        UrlRejection::NoHost => ClipError::InvalidUrl("The web address has no host".to_string()),
        UrlRejection::TrailingDot => {
            ClipError::InvalidUrl("The web address host must not end with a dot".to_string())
        }
        UrlRejection::LocalName | UrlRejection::PrivateAddress => {
            ClipError::Blocked("Local and private web addresses cannot be clipped".to_string())
        }
    })
}

/// Clip a page, reporting the article and the address it actually came from.
///
/// The address returned is where the fetch ended, not where it began. Relative links in the
/// article resolve against it, so a page reached through a redirect would otherwise have
/// every one of its links rewritten to a path on the wrong page.
pub fn clip_url(raw_url: &str) -> Result<(ClippedArticle, String), ClipError> {
    let requested_url = validate_source_url(raw_url)?.to_string();
    let (html, final_url) = fetch_with(&fetch_public_html, &requested_url)?;
    let final_url = final_url.to_string();
    let article = extract_article(&html, &final_url)?;
    Ok((article, final_url))
}

fn fetch_with<Source>(source: &Source, raw_url: &str) -> Result<(String, Url), ClipError>
where
    Source: Fn(&Url) -> Result<(String, Url), SourceError>,
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
    safe_fetch::resolve_public_addrs(url).map_err(|rejection| match rejection {
        HostRejection::NoHost => {
            SourceError::Unreachable("The web address has no host".to_string())
        }
        HostRejection::NoPort => {
            SourceError::Unreachable("The web address has no port".to_string())
        }
        HostRejection::Unresolvable(_) => {
            SourceError::Unreachable("The web page host could not be reached".to_string())
        }
        HostRejection::NoAddresses => {
            SourceError::Unreachable("The web page host resolved to no addresses".to_string())
        }
        HostRejection::PrivateAddress => SourceError::Blocked(
            "The web page host resolves to a local or private address".to_string(),
        ),
    })
}

/// Redirects are refused at the client and handled by hand, because each hop has to be
/// revalidated before it is followed — a public page may redirect to a private address.
fn html_client(host: &str, addresses: &[SocketAddr]) -> Result<Client, SourceError> {
    safe_fetch::pinned_client(host, addresses, Duration::from_secs(20))
        .redirect(Policy::none())
        .build()
        .map_err(|_| SourceError::Unreachable("The web request could not be prepared".to_string()))
}

fn fetch_public_html(url: &Url) -> Result<(String, Url), SourceError> {
    let mut current_url = url.clone();
    for redirect_count in 0..=5 {
        let html = fetch_public_html_without_redirects(&current_url, redirect_count)?;
        match html {
            FetchStep::Html(html) => return Ok((html, current_url)),
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

    let body = safe_fetch::read_capped(&mut response, MAX_ARTICLE_BYTES)
        .map_err(|_| SourceError::Unreachable("The web page could not be read".to_string()))?;
    let body = match body {
        Body::Complete(body) => body,
        Body::TooLarge => {
            return Err(SourceError::Blocked(
                "The web page exceeds the 5 MiB clipping limit".to_string(),
            ))
        }
    };
    let html = String::from_utf8(body)
        .map_err(|_| SourceError::Blocked("The web page is not valid UTF-8 HTML".to_string()))?;
    if let Some(target) = meta_refresh_target(&html) {
        if redirect_count >= 5 {
            return Err(SourceError::Blocked(
                "The web page redirected too many times".to_string(),
            ));
        }
        return Ok(FetchStep::Redirect(redirect_target(url, &target)?));
    }
    Ok(FetchStep::Html(html))
}

/// Where a page redirects by `<meta http-equiv="refresh">`, if it does.
///
/// Sites publish these as plain HTML stubs — a title, a script, and a `<noscript>` fallback
/// — so a fetcher that only understands 3xx sees a few hundred bytes of nothing and reports
/// no readable article for a URL that is perfectly good. Following it costs one hop from the
/// same budget, and the destination is validated like any other.
///
/// Only an immediate refresh counts. A page that sets a long delay is reloading itself on a
/// timer, which is not a redirect and must not drag the clipper somewhere else.
fn meta_refresh_target(html: &str) -> Option<String> {
    for tag in html_tags(html, "meta") {
        if !tag_attribute(&tag, "http-equiv")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("refresh"))
        {
            continue;
        }
        // Every step below moves on to the next tag rather than abandoning the scan: a page
        // may carry a refresh that is not a redirect — no destination, or a long delay —
        // ahead of one that is.
        let Some(content) = tag_attribute(&tag, "content") else {
            continue;
        };
        let Some((delay, target)) = content.split_once(';') else {
            continue;
        };
        // A delay that will not parse is not an instruction to go anywhere, and one longer
        // than a moment is a page reloading itself rather than redirecting.
        if !delay.trim().parse::<f32>().is_ok_and(|delay| delay <= 1.0) {
            continue;
        }
        let target = target.trim();
        let Some(target) = target
            .get(..4)
            .filter(|prefix| prefix.eq_ignore_ascii_case("url="))
            .map(|_| &target[4..])
        else {
            continue;
        };
        let target = target.trim().trim_matches(['"', '\''].as_slice());
        if !target.is_empty() {
            return Some(target.to_string());
        }
    }
    None
}

/// The text of every `<name ...>` tag in the document, without a full HTML parse.
fn html_tags(html: &str, name: &str) -> Vec<String> {
    let opener = format!("<{name}");
    let lower = html.to_ascii_lowercase();
    let mut tags = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find(&opener) {
        let start = cursor + offset;
        // Reject <metadata> and friends: the name has to end where the tag says it does.
        let after = lower[start + opener.len()..].chars().next();
        if after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-') {
            cursor = start + opener.len();
            continue;
        }
        let end = html[start..]
            .find('>')
            .map_or(html.len(), |offset| start + offset);
        tags.push(html[start..end].to_string());
        cursor = end.max(start + opener.len());
    }
    tags
}

/// An attribute's value from a single tag's text, quoted or bare.
fn tag_attribute(tag: &str, attribute: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find(attribute) {
        let start = cursor + offset;
        cursor = start + attribute.len();
        // The match has to be a whole attribute name, not the tail of another.
        if lower[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            continue;
        }
        let rest = tag[cursor..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => rest[1..].split(quote).next()?,
            _ => rest.split_whitespace().next()?,
        };
        return Some(value.to_string());
    }
    None
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

/// Distil a fetched page into a note body.
///
/// Readable content decides first, and a login wall is only ever diagnosed for a page that
/// produced no article. Checking for the wall up front reads the furniture every ordinary
/// article carries — a search form, a "Log in" link — as the wall itself.
pub fn extract_article(html: &str, source_url: &str) -> Result<ClippedArticle, ClipError> {
    let article =
        legible::parse(html, Some(source_url), None).map_err(|_| unreadable_page_error(html))?;
    if article.text_content.split_whitespace().count() < MIN_ARTICLE_WORDS {
        return Err(unreadable_page_error(html));
    }
    Ok(ClippedArticle {
        title: article.title.trim().to_string(),
        markdown: article.markdown_content.trim().to_string(),
    })
}

/// Why a page yielded no article. A password field is the one cause we can name with
/// confidence; anything else is reported as what the user actually observes.
fn unreadable_page_error(html: &str) -> ClipError {
    if asks_for_a_password(html) {
        return ClipError::NotArticle(
            "The web page requires login before it can be clipped".to_string(),
        );
    }
    ClipError::NotArticle("No readable article content was found on that page".to_string())
}

fn asks_for_a_password(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("type=\"password\"")
        || lower.contains("type='password'")
        || lower.contains("type=password")
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

    /// Ordinary articles carry a search form and a sign-in link in their furniture.
    /// Wikipedia and most blogs do, so reading those as a wall rejects most of the web.
    #[test]
    fn clips_an_article_whose_furniture_offers_a_login_link() {
        let html = r#"
            <!doctype html>
            <html>
              <head><title>Zettelkasten</title></head>
              <body>
                <nav>
                  <form action="/search"><input type="search" name="q"></form>
                  <a href="/login">Log in</a>
                  <a href="/signup">Create account</a>
                </nav>
                <main>
                  <article>
                    <h1>Zettelkasten</h1>
                    <p>A zettelkasten is a method of personal knowledge management built
                       from many small notes that each hold a single idea.</p>
                    <p>Because every note is linked to the others it belongs beside, the
                       collection grows into a structure the writer did not plan in advance.</p>
                  </article>
                </main>
              </body>
            </html>
        "#;

        let article = extract_article(html, "https://example.org/wiki/Zettelkasten")
            .expect("a login link in the navigation is not a login wall");

        assert!(article.markdown.contains("single idea"));
        assert!(!article.markdown.contains("Create account"));
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
                    clip_url(url),
                    Err(ClipError::InvalidUrl(_) | ClipError::Blocked(_))
                ),
                "accepted {url}"
            );
        }
    }

    #[test]
    fn reports_a_timeout_as_a_distinct_actionable_failure() {
        let source = |_url: &Url| Err(SourceError::Timeout);

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

    /// The fixtures above are written by the same person as the parser call, so they cannot
    /// show what real pages do to it. This clips live articles chosen for the furniture that
    /// broke the earlier heuristic: a search form plus a sign-in link.
    #[test]
    #[ignore = "needs network access to third-party sites"]
    fn clips_live_articles_from_the_public_web() {
        for url in [
            "https://en.wikipedia.org/wiki/Zettelkasten",
            "https://blog.rust-lang.org/2024/02/08/Rust-1.76.0/",
            // Serves a meta-refresh stub rather than the post itself.
            "https://blog.rust-lang.org/2024/02/08/Rust-1.76.0.html",
        ] {
            let (article, canonical_url) = clip_url(url).unwrap_or_else(|error| {
                panic!("clipping {url} failed: {}", error.message());
            });

            assert!(
                canonical_url.starts_with("https://"),
                "{url} -> {canonical_url}"
            );
            assert!(!article.title.is_empty(), "{url} produced no title");
            assert!(
                article.markdown.split_whitespace().count() > 100,
                "{url} produced only {} words",
                article.markdown.split_whitespace().count()
            );
        }
    }

    /// An unreachable host must surface as a fetch failure, not a panic or a hang.
    #[test]
    #[ignore = "needs network access to resolve a nonexistent host"]
    fn reports_an_unreachable_host_without_producing_an_article() {
        let error = clip_url("https://this-host-does-not-exist.invalid/article")
            .expect_err("a nonexistent host cannot be clipped");

        assert!(matches!(error, ClipError::Fetch(_) | ClipError::Blocked(_)));
        assert!(!error.message().is_empty());
    }

    /// The exact stub blog.rust-lang.org serves for a renamed post: no article, a script,
    /// and a noscript meta refresh carrying the real address.
    #[test]
    fn follows_a_meta_refresh_stub_to_the_real_article() {
        let html = r#"
            <!doctype html>
            <meta charset="utf-8">
            <title>Redirect</title>
            <script>
              const target = "https://blog.example.org/2024/02/08/Release/";
              window.location.replace(target);
            </script>
            <noscript>
              <meta http-equiv="refresh" content="0; url=https://blog.example.org/2024/02/08/Release/">
            </noscript>
            <p><a href="https://blog.example.org/2024/02/08/Release/">Click here</a>.</p>
        "#;

        assert_eq!(
            meta_refresh_target(html).as_deref(),
            Some("https://blog.example.org/2024/02/08/Release/")
        );
    }

    #[test]
    fn reads_a_meta_refresh_however_it_is_written() {
        for (html, expected) in [
            (
                r#"<meta http-equiv="REFRESH" content="0;URL='/next'">"#,
                Some("/next"),
            ),
            (
                r#"<meta content=0;url=/bare http-equiv=refresh>"#,
                Some("/bare"),
            ),
            (
                r#"<meta http-equiv='refresh' content='1; url=/soon'>"#,
                Some("/soon"),
            ),
        ] {
            assert_eq!(meta_refresh_target(html).as_deref(), expected, "{html}");
        }
    }

    /// A refresh that is not a redirect must not hide one that is: each is skipped so the
    /// scan continues, rather than abandoning the document.
    #[test]
    fn a_non_redirect_refresh_does_not_hide_a_later_real_one() {
        let html = r#"
            <meta http-equiv="refresh" content="5">
            <meta http-equiv="refresh" content="600; url=/reload-loop">
            <meta http-equiv="refresh">
            <meta http-equiv="refresh" content="0; url=/the-article">
        "#;

        assert_eq!(meta_refresh_target(html).as_deref(), Some("/the-article"));
    }

    /// A page reloading itself on a timer is not redirecting, and a clipper that treats it
    /// as one walks away from the article the user asked for.
    #[test]
    fn ignores_refreshes_that_are_not_redirects() {
        for html in [
            r#"<meta http-equiv="refresh" content="30; url=/dashboard">"#,
            r#"<meta http-equiv="refresh" content="5">"#,
            r#"<meta http-equiv="refresh" content="not-a-number; url=/nowhere">"#,
            r#"<meta http-equiv="content-type" content="0; url=/elsewhere">"#,
            r#"<metadata http-equiv="refresh" content="0; url=/elsewhere">"#,
            r#"<p>no meta here</p>"#,
        ] {
            assert_eq!(meta_refresh_target(html), None, "{html}");
        }
    }

    #[test]
    fn rejects_redirect_targets_to_private_addresses() {
        let current = Url::parse("https://example.com/story").unwrap();
        let error = redirect_target(&current, "http://127.0.0.1/story")
            .expect_err("private redirects stay blocked");

        assert!(matches!(error, SourceError::Blocked(_)));
    }
}
