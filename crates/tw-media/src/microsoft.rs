//! Microsoft's Windows 11 download page and its connector API.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tw_core::host::Arch;
use tw_core::{ErrorCode, TwError, TwResult};

const CONNECTOR: &str = "https://www.microsoft.com/software-download-connector/api";
/// Public profile id embedded in Microsoft's download page script.
const PROFILE: &str = "606624d44113";
const USER_AGENT: &str = concat!("terminal-windows/", env!("CARGO_PKG_VERSION"));

pub fn page_url(arch: Arch) -> &'static str {
    match arch {
        Arch::X86_64 => "https://www.microsoft.com/en-us/software-download/windows11",
        Arch::Aarch64 => "https://www.microsoft.com/en-us/software-download/windows11arm64",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishedHash {
    /// Row label as published, e.g. "Korean 64-bit".
    pub label: String,
    /// Language part of the label, e.g. "Korean".
    pub language: String,
    /// Lower-case hex SHA-256.
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageInfo {
    pub arch: Arch,
    pub url: String,
    pub edition_id: String,
    pub edition_name: String,
    pub version: Option<String>,
    pub hashes: Vec<PublishedHash>,
}

impl PageInfo {
    pub fn hash_for(&self, language: &str) -> Option<&PublishedHash> {
        self.hashes.iter().find(|h| same_language(&h.language, language))
    }
    pub fn match_hash(&self, sha256: &str) -> Option<&PublishedHash> {
        self.hashes.iter().find(|h| h.sha256.eq_ignore_ascii_case(sha256))
    }
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parses the download page HTML. Pure function, tested with fixtures.
pub fn parse_page(html: &str, arch: Arch) -> TwResult<PageInfo> {
    let bad = |what: &str| {
        TwError::new(ErrorCode::Backend, format!("Microsoft's download page changed: {what} not found"))
            .with_hint(format!("Download manually from {} and use `terminal-windows image import PATH`.", page_url(arch)))
    };
    // Edition: <option value="3816">Windows 11 (multi-edition ISO for Arm64)</option>
    let mut edition = None;
    for part in html.split("<option value=\"").skip(1) {
        let Some((id, rest)) = part.split_once('"') else { continue };
        if !id.chars().all(|c| c.is_ascii_digit()) || id.is_empty() {
            continue;
        }
        let label = rest.split_once('>').map(|(_, r)| r.split('<').next().unwrap_or("")).unwrap_or("").trim().to_string();
        if label.starts_with("Windows 11") {
            edition = Some((id.to_string(), label));
            break;
        }
    }
    let (edition_id, edition_name) = edition.ok_or_else(|| bad("edition selector"))?;

    // Hash table rows: <td>Korean 64-bit</td><td>HEX64</td>
    let mut hashes = Vec::new();
    let cells: Vec<String> = html
        .split("<td")
        .skip(1)
        .filter_map(|c| c.split_once('>').map(|(_, r)| strip_tags(r.split("</td>").next().unwrap_or(""))))
        .collect();
    for pair in cells.windows(2) {
        let (label, hash) = (&pair[0], &pair[1]);
        if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) && !label.is_empty() {
            let language = label.trim_end_matches("64-bit").trim().to_string();
            hashes.push(PublishedHash { label: label.clone(), language, sha256: hash.to_ascii_lowercase() });
        }
    }
    if hashes.is_empty() {
        return Err(bad("SHA-256 table"));
    }
    let version = html.find("Version ").and_then(|i| {
        let v: String = html[i + 8..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        (v.len() >= 3 && v.len() <= 8).then_some(v)
    });
    Ok(PageInfo { arch, url: page_url(arch).to_string(), edition_id, edition_name, version, hashes })
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Sku {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Language")]
    pub language: String,
    #[serde(rename = "LocalizedLanguage", default)]
    pub localized: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkOutcome {
    /// A time-limited download URL granted by Microsoft.
    Granted(String),
    /// Microsoft declined automated access (bot detection or region block).
    Rejected(String),
}

#[derive(Deserialize)]
struct ApiErrors {
    #[serde(rename = "Errors", default)]
    errors: Option<Vec<ApiError>>,
}
#[derive(Deserialize)]
struct ApiError {
    #[serde(rename = "Key", default)]
    key: String,
    #[serde(rename = "Value", default)]
    value: String,
}

#[derive(Deserialize)]
struct SkuResponse {
    #[serde(rename = "Skus", default)]
    skus: Vec<Sku>,
}

#[derive(Deserialize)]
struct LinksResponse {
    #[serde(rename = "ProductDownloadOptions", default)]
    options: Vec<LinkOption>,
}
#[derive(Deserialize)]
struct LinkOption {
    #[serde(rename = "Uri")]
    uri: String,
}

/// Interprets the link API response. Pure, tested.
pub fn parse_links(json: &str) -> TwResult<LinkOutcome> {
    let errors: ApiErrors = serde_json::from_str(json).map_err(|e| TwError::backend(format!("unexpected link response: {e}")))?;
    if let Some(e) = errors.errors.and_then(|v| v.into_iter().next()) {
        return Ok(LinkOutcome::Rejected(format!("{} ({})", e.value, e.key)));
    }
    let links: LinksResponse = serde_json::from_str(json).map_err(|e| TwError::backend(format!("unexpected link response: {e}")))?;
    let uri = links
        .options
        .into_iter()
        .map(|o| o.uri)
        .find(|u| u.starts_with("https://"))
        .ok_or_else(|| TwError::backend("Microsoft returned no download link"))?;
    Ok(LinkOutcome::Granted(uri))
}

pub struct Client {
    http: reqwest::Client,
}

impl Client {
    pub fn new() -> TwResult<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| TwError::backend(format!("HTTP client: {e}")))?;
        Ok(Self { http })
    }

    async fn get_text(&self, url: &str, referer: Option<&str>) -> TwResult<String> {
        let mut req = self.http.get(url);
        if let Some(r) = referer {
            req = req.header(reqwest::header::REFERER, r);
        }
        let resp = req.send().await.map_err(|e| TwError::new(ErrorCode::Io, format!("GET {url}: {e}")))?;
        if !resp.status().is_success() {
            return Err(TwError::backend(format!("GET {url}: HTTP {}", resp.status())));
        }
        resp.text().await.map_err(|e| TwError::new(ErrorCode::Io, format!("GET {url}: {e}")))
    }

    pub async fn page(&self, arch: Arch) -> TwResult<PageInfo> {
        parse_page(&self.get_text(page_url(arch), None).await?, arch)
    }

    pub async fn skus(&self, page: &PageInfo, session: &str) -> TwResult<Vec<Sku>> {
        // Registers the session the same way the page does.
        let _ = self.get_text(&format!("https://vlscppe.microsoft.com/tags?org_id=y6jn8c31&session_id={session}"), Some(&page.url)).await;
        let url = format!(
            "{CONNECTOR}/getskuinformationbyproductedition?profile={PROFILE}&ProductEditionId={}&SKU=undefined&friendlyFileName=undefined&Locale=en-US&sessionID={session}",
            page.edition_id
        );
        let text = self.get_text(&url, Some(&page.url)).await?;
        let skus: SkuResponse = serde_json::from_str(&text).map_err(|e| TwError::backend(format!("unexpected SKU response: {e}")))?;
        if skus.skus.is_empty() {
            return Err(TwError::backend("Microsoft returned no languages for this edition"));
        }
        Ok(skus.skus)
    }

    pub async fn link(&self, page: &PageInfo, sku: &Sku, session: &str) -> TwResult<LinkOutcome> {
        let url = format!(
            "{CONNECTOR}/GetProductDownloadLinksBySku?profile={PROFILE}&ProductEditionId=undefined&SKU={}&friendlyFileName=undefined&Locale=en-US&sessionID={session}",
            sku.id
        );
        parse_links(&self.get_text(&url, Some(&page.url)).await?)
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }
}

/// Microsoft names languages differently in the hash table ("Chinese Simplified",
/// "English International") and in the SKU API ("Chinese (Simplified)",
/// "English (United Kingdom)"). Compare through this key.
pub fn language_key(name: &str) -> String {
    let key: String = name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
    match key.as_str() {
        "englishunitedkingdom" => "englishinternational".into(),
        _ => key,
    }
}

pub fn same_language(a: &str, b: &str) -> bool {
    language_key(a) == language_key(b)
}

pub fn session_id() -> String {
    tw_core::model::random_uuid()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = include_str!("../tests/fixtures/windows11arm64-page.html");

    #[test]
    fn parses_edition_version_and_hashes() {
        let p = parse_page(PAGE, Arch::Aarch64).unwrap();
        assert_eq!(p.edition_id, "3816");
        assert_eq!(p.edition_name, "Windows 11 (multi-edition ISO for Arm64)");
        assert_eq!(p.version.as_deref(), Some("26H2"));
        assert_eq!(p.hashes.len(), 3);
        let ko = p.hash_for("korean").unwrap();
        assert_eq!(ko.sha256, "833e132e94335a7eecc2b3bb6ba9e16b199413504049cc19743752f402fb9cf7");
        assert_eq!(p.match_hash(&ko.sha256.to_uppercase()).unwrap().language, "Korean");
    }

    #[test]
    fn changed_page_gives_actionable_error() {
        let err = parse_page("<html>redesigned</html>", Arch::X86_64).unwrap_err();
        assert!(err.hint.unwrap().contains("image import"));
    }

    #[test]
    fn language_names_from_both_sources_match() {
        assert!(same_language("Chinese Simplified", "Chinese (Simplified)"));
        assert!(same_language("English International", "English (United Kingdom)"));
        assert!(same_language("korean", "Korean"));
        assert!(!same_language("English", "English International"));
    }

    #[test]
    fn link_response_granted_or_rejected() {
        let ok = r#"{"ProductDownloadOptions":[{"Uri":"https://software.download.prss.microsoft.com/x/Win11_26H2_English_Arm64.iso?t=1","DownloadType":0}]}"#;
        assert!(matches!(parse_links(ok).unwrap(), LinkOutcome::Granted(u) if u.contains("Win11_26H2")));
        let rejected =
            r#"{"Errors":[{"Key":"ErrorSettings.SentinelReject","Value":"Sentinel marked this request as rejected.","Type":8}]}"#;
        assert!(matches!(parse_links(rejected).unwrap(), LinkOutcome::Rejected(m) if m.contains("Sentinel")));
        assert!(parse_links(r#"{"ProductDownloadOptions":[{"Uri":"http://insecure"}]}"#).is_err());
    }
}
