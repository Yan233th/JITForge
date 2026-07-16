use std::{collections::BTreeSet, net::IpAddr, time::Duration};

use jit_protocol::HttpCapability;
use reqwest::{Url, header};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::net::lookup_host;

const SEARCH_RESPONSE_LIMIT: usize = 512 * 1024;
const DOCUMENT_RESPONSE_LIMIT: usize = 256 * 1024;
const PROBE_RESPONSE_LIMIT: usize = 1024 * 1024;
const MAX_REDIRECTS: usize = 3;

#[derive(Clone)]
pub struct WebAccess {
    search_base_url: Url,
    search_client: reqwest::Client,
    search_engines: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FetchedDocument {
    pub url: String,
    pub content_type: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct ProbedResponse {
    pub url: String,
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

#[derive(Deserialize)]
struct SearxResponse {
    #[serde(default)]
    results: Vec<SearxResult>,
}

#[derive(Deserialize)]
struct SearxResult {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default, alias = "snippet")]
    content: String,
}

impl WebAccess {
    pub fn new(provider: &str, base_url: &str, engines: &str) -> Result<Self, WebAccessError> {
        if provider != "searxng" {
            return Err(WebAccessError::InvalidConfig(format!(
                "unsupported search provider {provider:?}; expected searxng"
            )));
        }
        let search_base_url = Url::parse(base_url)
            .map_err(|error| WebAccessError::InvalidConfig(error.to_string()))?;
        if !matches!(search_base_url.scheme(), "http" | "https")
            || search_base_url.host_str().is_none()
            || !search_base_url.username().is_empty()
            || search_base_url.password().is_some()
        {
            return Err(WebAccessError::InvalidConfig(
                "search.base_url must be an HTTP(S) URL without credentials".to_owned(),
            ));
        }
        let search_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .user_agent("JITForge/0.1 search")
            .build()
            .map_err(|error| WebAccessError::InvalidConfig(error.to_string()))?;
        Ok(Self {
            search_base_url,
            search_client,
            search_engines: engines.trim().to_owned(),
        })
    }

    pub async fn search(&self, query: &str) -> Result<Vec<WebSearchResult>, WebAccessError> {
        if query.trim().is_empty() || query.len() > 512 {
            return Err(WebAccessError::InvalidRequest(
                "search query must contain 1-512 bytes".to_owned(),
            ));
        }
        let mut url = self
            .search_base_url
            .join("search")
            .map_err(|error| WebAccessError::InvalidConfig(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("q", query.trim())
            .append_pair("format", "json")
            .append_pair("categories", "general")
            .append_pair("safesearch", "1");
        if !self.search_engines.is_empty() {
            url.query_pairs_mut()
                .append_pair("engines", &self.search_engines);
        }
        let response = self.search_client.get(url).send().await?;
        if !response.status().is_success() {
            return Err(WebAccessError::Upstream(format!(
                "SearXNG returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let body = read_bounded(response, SEARCH_RESPONSE_LIMIT).await?;
        let payload: SearxResponse = serde_json::from_slice(&body)
            .map_err(|error| WebAccessError::Upstream(error.to_string()))?;
        Ok(payload
            .results
            .into_iter()
            .filter(|result| Url::parse(&result.url).is_ok())
            .take(8)
            .map(|result| WebSearchResult {
                title: truncate(result.title, 512),
                url: truncate(result.url, 2048),
                snippet: truncate(result.content, 2048),
            })
            .collect())
    }

    pub async fn fetch_document(&self, url: &str) -> Result<FetchedDocument, WebAccessError> {
        let response = self.public_get(url, DOCUMENT_RESPONSE_LIMIT, None).await?;
        if !(200..300).contains(&response.status) {
            return Err(WebAccessError::Upstream(format!(
                "document returned HTTP {}",
                response.status
            )));
        }
        let text = String::from_utf8_lossy(&response.body).into_owned();
        let text = if response.content_type.contains("html") {
            strip_html(&text)
        } else {
            text
        };
        Ok(FetchedDocument {
            url: response.url,
            content_type: response.content_type,
            text: truncate(text, 64 * 1024),
        })
    }

    pub async fn probe(
        &self,
        url: &str,
        capability: &HttpCapability,
    ) -> Result<ProbedResponse, WebAccessError> {
        self.public_get(url, PROBE_RESPONSE_LIMIT, Some(capability))
            .await
    }

    async fn public_get(
        &self,
        raw_url: &str,
        limit: usize,
        capability: Option<&HttpCapability>,
    ) -> Result<ProbedResponse, WebAccessError> {
        let mut url = validate_public_url(raw_url)?;
        for redirects in 0..=MAX_REDIRECTS {
            if capability.is_some_and(|capability| !capability_allows(capability, &url)) {
                return Err(WebAccessError::UnsafeUrl(
                    "URL is outside the approved HTTP capability".to_owned(),
                ));
            }
            let host = url
                .host_str()
                .ok_or_else(|| WebAccessError::UnsafeUrl("URL has no host".to_owned()))?;
            let port = url.port_or_known_default().unwrap_or(443);
            let addresses = lookup_host((host, port)).await?.collect::<BTreeSet<_>>();
            if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
                return Err(WebAccessError::UnsafeUrl(
                    "hostname resolved to a non-public address".to_owned(),
                ));
            }
            let pinned = *addresses.iter().next().expect("non-empty checked above");
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .resolve(host, pinned)
                .user_agent("JITForge/0.1 synthesis-probe")
                .build()?;
            let response = client
                .get(url.clone())
                .header(
                    header::ACCEPT,
                    "application/json,text/plain,text/html;q=0.8,*/*;q=0.2",
                )
                .send()
                .await?;
            if response.status().is_redirection() {
                if redirects == MAX_REDIRECTS {
                    return Err(WebAccessError::RedirectLimit);
                }
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        WebAccessError::UnsafeUrl("redirect omitted Location".to_owned())
                    })?;
                url = validate_public_url(url.join(location)?.as_str())?;
                continue;
            }
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("application/octet-stream")
                .split(';')
                .next()
                .unwrap_or("application/octet-stream")
                .trim()
                .to_owned();
            let body = read_bounded(response, limit).await?;
            return Ok(ProbedResponse {
                url: url.to_string(),
                status,
                content_type,
                body,
            });
        }
        Err(WebAccessError::RedirectLimit)
    }
}

fn capability_allows(capability: &HttpCapability, url: &Url) -> bool {
    let query_keys = url
        .query_pairs()
        .map(|(key, _)| key.into_owned())
        .collect::<BTreeSet<_>>();
    url.scheme() == capability.scheme
        && url.host_str().is_some_and(|host| {
            capability
                .host
                .eq_ignore_ascii_case(host.trim_end_matches('.'))
        })
        && url.port_or_known_default() == Some(capability.port)
        && url.path().starts_with(&capability.path_prefix)
        && query_keys
            .iter()
            .all(|key| capability.query_keys.contains(key))
}

async fn read_bounded(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, WebAccessError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(WebAccessError::ResponseTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(WebAccessError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn validate_public_url(raw: &str) -> Result<Url, WebAccessError> {
    if raw.len() > 4096 {
        return Err(WebAccessError::UnsafeUrl("URL is too long".to_owned()));
    }
    let url = Url::parse(raw)?;
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(WebAccessError::UnsafeUrl(
            "only credential-free HTTPS URLs on port 443 are allowed".to_owned(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| WebAccessError::UnsafeUrl("URL has no host".to_owned()))?;
    if host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok() {
        return Err(WebAccessError::UnsafeUrl(
            "IP literals and localhost are not allowed".to_owned(),
        ));
    }
    Ok(url)
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || octets[0] == 0
                || octets[0] >= 240
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
                || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
                || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113))
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| !is_public_ip(mapped.into())))
        }
    }
}

fn strip_html(input: &str) -> String {
    let mut output = String::with_capacity(input.len().min(64 * 1024));
    let mut in_tag = false;
    for character in input.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                if !output.ends_with(char::is_whitespace) {
                    output.push(' ');
                }
            }
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let boundary = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= limit)
        .last()
        .unwrap_or(0);
    value.truncate(boundary);
    value
}

#[derive(Debug, Error)]
pub enum WebAccessError {
    #[error("invalid web access configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid web access request: {0}")]
    InvalidRequest(String),
    #[error("unsafe URL: {0}")]
    UnsafeUrl(String),
    #[error("web response exceeded its size limit")]
    ResponseTooLarge,
    #[error("web redirect limit exceeded")]
    RedirectLimit,
    #[error("web upstream failed: {0}")]
    Upstream(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_and_documentation_networks() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "::1",
            "fd00::1",
            "2001:db8::1",
        ] {
            assert!(!is_public_ip(address.parse().unwrap()), "{address}");
        }
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn public_urls_require_https_dns_names() {
        assert!(validate_public_url("https://example.com/api").is_ok());
        assert!(validate_public_url("http://example.com/api").is_err());
        assert!(validate_public_url("https://127.0.0.1/api").is_err());
        assert!(validate_public_url("https://user@example.com/api").is_err());
    }

    #[test]
    fn html_is_reduced_to_bounded_readable_text() {
        assert_eq!(strip_html("<h1>Hello</h1><p>world</p>"), "Hello world");
    }
}
