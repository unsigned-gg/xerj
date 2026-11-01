//! GitHub Releases API client. Authentication is optional: the fork is public,
//! so an unauthenticated client works; a token only lifts rate limits and lets a
//! private mirror resolve.

use anyhow::{bail, Context, Result};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    /// The API URL of the asset (`.../releases/assets/<id>`), not the browser URL.
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// `GITHUB_TOKEN`, then `GH_TOKEN`, then `gh auth token`. `None` is fine.
pub fn resolve_token() -> Option<String> {
    for var in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    let out = Command::new("gh")
        .args(["auth", "token"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!t.is_empty()).then_some(t)
}

pub struct Client {
    http: reqwest::blocking::Client,
    api_base: reqwest::Url,
    repo: String,
    token: Option<String>,
}

impl Client {
    pub fn new(api_base: &str, repo: &str, token: Option<String>) -> Result<Client> {
        let http = reqwest::blocking::Client::builder()
            .user_agent(concat!("xerj-update/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(300))
            .build()
            .context("cannot build the HTTP client")?;
        let api_base =
            reqwest::Url::parse(api_base).with_context(|| format!("bad API base '{api_base}'"))?;
        Ok(Client {
            http,
            api_base,
            repo: repo.to_string(),
            token,
        })
    }

    fn same_origin(&self, url: &reqwest::Url) -> bool {
        url.scheme() == self.api_base.scheme()
            && url.host_str() == self.api_base.host_str()
            && url.port_or_known_default() == self.api_base.port_or_known_default()
    }

    /// The token goes only to the API origin. A redirect to another host (the
    /// release object store) is followed by reqwest without the header.
    fn get(&self, url: reqwest::Url, accept: &str) -> Result<reqwest::blocking::Response> {
        let mut req = self
            .http
            .get(url.clone())
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let (Some(t), true) = (&self.token, self.same_origin(&url)) {
            req = req.bearer_auth(t);
        }
        let resp = req
            .send()
            .with_context(|| format!("request to {} failed", url.host_str().unwrap_or("?")))?;
        if !resp.status().is_success() {
            bail!("GET {} returned {}", url.path(), resp.status());
        }
        Ok(resp)
    }

    fn api_url(&self, path: &str) -> Result<reqwest::Url> {
        let base = self.api_base.as_str().trim_end_matches('/');
        reqwest::Url::parse(&format!("{base}/repos/{}/{path}", self.repo)).context("bad API url")
    }

    fn release_from(v: &serde_json::Value) -> Result<Release> {
        let tag = v
            .get("tag_name")
            .and_then(|x| x.as_str())
            .context("release has no tag_name")?
            .to_string();
        let mut assets = Vec::new();
        for a in v
            .get("assets")
            .and_then(|x| x.as_array())
            .map(|x| x.as_slice())
            .unwrap_or(&[])
        {
            if let (Some(name), Some(url)) = (
                a.get("name").and_then(|x| x.as_str()),
                a.get("url").and_then(|x| x.as_str()),
            ) {
                assets.push(Asset {
                    name: name.to_string(),
                    url: url.to_string(),
                    size: a.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                });
            }
        }
        Ok(Release { tag, assets })
    }

    pub fn release(&self, tag: &str) -> Result<Release> {
        let resp = self.get(
            self.api_url(&format!("releases/tags/{tag}"))?,
            "application/vnd.github+json",
        )?;
        let v: serde_json::Value = resp.json().context("release JSON is not valid")?;
        Self::release_from(&v)
    }

    /// Newest published `vX.Y.Z` release. The list also holds corpus-pack
    /// releases, so non-version tags are skipped; drafts are skipped.
    pub fn latest_tag(&self) -> Result<String> {
        let resp = self.get(
            self.api_url("releases?per_page=100")?,
            "application/vnd.github+json",
        )?;
        let v: serde_json::Value = resp.json().context("release list is not valid JSON")?;
        for r in v.as_array().context("release list is not an array")? {
            if r.get("draft").and_then(|x| x.as_bool()) == Some(true) {
                continue;
            }
            if let Some(tag) = r.get("tag_name").and_then(|x| x.as_str()) {
                if tag.starts_with('v') && crate::version::Version::parse(tag).is_ok() {
                    return Ok(tag.to_string());
                }
            }
        }
        bail!("no vX.Y.Z release found in {}", self.repo)
    }

    /// Download an asset through the API URL. `max` bounds the read.
    pub fn download(&self, asset: &Asset, max: u64) -> Result<Vec<u8>> {
        let url = reqwest::Url::parse(&asset.url)
            .with_context(|| format!("{}: bad asset url", asset.name))?;
        let resp = self.get(url, "application/octet-stream")?;
        let mut out = Vec::new();
        resp.take(max + 1)
            .read_to_end(&mut out)
            .with_context(|| format!("{}: download failed", asset.name))?;
        if out.len() as u64 > max {
            bail!("{}: larger than the {max}-byte limit", asset.name);
        }
        Ok(out)
    }
}
