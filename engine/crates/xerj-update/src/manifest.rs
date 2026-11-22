//! The release manifest `xerj-<version>-manifest.json`, and archive checks.
//!
//! The manifest records, per target, the sha256 and byte size of the ARCHIVE its
//! URL points at (not of the binary inside it) and the commit the tag points at.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub target: String,
    pub binary: String,
    pub archive: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub tag: String,
    pub tag_commit: String,
    pub artifacts: Vec<Artifact>,
}

pub fn manifest_name(version: &str) -> String {
    format!("xerj-{version}-manifest.json")
}

pub fn archive_name(version: &str, target: &str) -> String {
    let ext = if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("xerj-{version}-{target}.{ext}")
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn text<'a>(v: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(|x| x.as_str())
        .with_context(|| format!("manifest: '{key}' is missing or not a string"))
}

/// Parse and validate a manifest for `expected_tag`.
pub fn parse(bytes: &[u8], expected_tag: &str) -> Result<Manifest> {
    let v: serde_json::Value =
        serde_json::from_slice(bytes).context("manifest is not valid JSON")?;
    if text(&v, "package")? != "xerj" {
        bail!("manifest: package is not xerj");
    }
    if text(&v, "channel")? != "stable" {
        bail!("manifest: channel is not stable");
    }
    let tag = text(&v, "tag")?;
    if tag != expected_tag {
        bail!("manifest is for tag {tag}, not {expected_tag}");
    }
    let version = text(&v, "version")?;
    if Some(version) != expected_tag.strip_prefix('v') {
        bail!("manifest: version {version} does not match tag {expected_tag}");
    }
    let tag_commit = text(&v, "tag_commit")?;
    if !is_hex(tag_commit, 40) {
        bail!("manifest: tag_commit is not a 40-character lowercase hex sha");
    }
    let mut artifacts = Vec::new();
    for a in v
        .get("artifacts")
        .and_then(|x| x.as_array())
        .context("manifest: 'artifacts' is missing")?
    {
        let target = text(a, "target")?.to_string();
        let archive = text(a, "archive")?.to_string();
        if archive != archive_name(version, &target) {
            bail!("manifest: {archive} is not the archive name for {target}");
        }
        let sha256 = text(a, "sha256")?.to_string();
        if !is_hex(&sha256, 64) {
            bail!("manifest: {archive} sha256 is not 64 lowercase hex characters");
        }
        let size = a
            .get("size")
            .and_then(|x| x.as_u64())
            .filter(|n| *n > 0)
            .with_context(|| format!("manifest: {archive} size is missing or zero"))?;
        artifacts.push(Artifact {
            target,
            binary: text(a, "binary")?.to_string(),
            archive,
            sha256,
            size,
        });
    }
    Ok(Manifest {
        version: version.to_string(),
        tag: tag.to_string(),
        tag_commit: tag_commit.to_string(),
        artifacts,
    })
}

impl Manifest {
    pub fn artifact_for(&self, target: &str) -> Option<&Artifact> {
        self.artifacts.iter().find(|a| a.target == target)
    }
}

/// The downloaded archive must be the one the manifest describes: byte size and
/// sha256 of the archive itself.
pub fn verify_archive(bytes: &[u8], artifact: &Artifact) -> Result<()> {
    if bytes.len() as u64 != artifact.size {
        bail!(
            "{}: downloaded {} bytes, manifest says {}",
            artifact.archive,
            bytes.len(),
            artifact.size
        );
    }
    let got = sha256_hex(bytes);
    if got != artifact.sha256 {
        bail!(
            "{}: sha256 {got} does not match the manifest ({})",
            artifact.archive,
            artifact.sha256
        );
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    pub fn manifest_json(version: &str, target: &str, archive: &[u8]) -> String {
        serde_json::json!({
            "package": "xerj", "channel": "stable", "version": version,
            "tag": format!("v{version}"), "tag_commit": COMMIT,
            "artifacts": [{
                "target": target, "binary": "xerj",
                "archive": archive_name(version, target),
                "url": "https://example.invalid/x",
                "sha256": sha256_hex(archive), "size": archive.len(),
            }]
        })
        .to_string()
    }

    #[test]
    fn parses_a_valid_manifest() {
        let j = manifest_json("1.2.3", "x86_64-unknown-linux-musl", b"archive-bytes");
        let m = parse(j.as_bytes(), "v1.2.3").unwrap();
        let a = m.artifact_for("x86_64-unknown-linux-musl").unwrap();
        assert_eq!(a.archive, "xerj-1.2.3-x86_64-unknown-linux-musl.tar.gz");
        assert_eq!(m.tag_commit, COMMIT);
        assert!(m.artifact_for("aarch64-apple-darwin").is_none());
    }

    #[test]
    fn rejects_the_wrong_tag() {
        let j = manifest_json("1.2.3", "x86_64-unknown-linux-musl", b"a");
        assert!(parse(j.as_bytes(), "v1.2.4").is_err());
    }

    #[test]
    fn rejects_a_bad_tag_commit() {
        for bad in ["main", "0123456", &COMMIT.to_uppercase(), ""] {
            let mut v: serde_json::Value =
                serde_json::from_str(&manifest_json("1.2.3", "x86_64-apple-darwin", b"a")).unwrap();
            v["tag_commit"] = bad.into();
            assert!(parse(v.to_string().as_bytes(), "v1.2.3").is_err(), "{bad}");
        }
    }

    #[test]
    fn rejects_an_archive_name_that_is_not_the_exact_name() {
        let mut v: serde_json::Value =
            serde_json::from_str(&manifest_json("1.2.3", "x86_64-apple-darwin", b"a")).unwrap();
        v["artifacts"][0]["archive"] = "xerj-1.2.3-x86_64-apple-darwin.tar.gz.evil".into();
        assert!(parse(v.to_string().as_bytes(), "v1.2.3").is_err());
    }

    #[test]
    fn rejects_garbage_and_missing_fields() {
        assert!(parse(b"not json", "v1.2.3").is_err());
        assert!(parse(b"{}", "v1.2.3").is_err());
    }

    #[test]
    fn archive_check_accepts_the_matching_archive() {
        let data = b"the archive";
        let j = manifest_json("1.2.3", "x86_64-apple-darwin", data);
        let m = parse(j.as_bytes(), "v1.2.3").unwrap();
        verify_archive(data, &m.artifacts[0]).unwrap();
    }

    #[test]
    fn archive_check_catches_size_and_hash_and_names_the_archive() {
        let data = b"the archive";
        let j = manifest_json("1.2.3", "x86_64-apple-darwin", data);
        let m = parse(j.as_bytes(), "v1.2.3").unwrap();
        let short = verify_archive(b"the archiv", &m.artifacts[0])
            .unwrap_err()
            .to_string();
        assert!(
            short.contains("xerj-1.2.3-x86_64-apple-darwin.tar.gz") && short.contains("bytes"),
            "{short}"
        );
        let flipped = verify_archive(b"the archivf", &m.artifacts[0])
            .unwrap_err()
            .to_string();
        assert!(flipped.contains("sha256"), "{flipped}");
    }

    #[test]
    fn the_hash_is_of_the_archive_not_of_the_binary() {
        // Patrick fix 1 shape: a manifest carrying the binary's hash must fail.
        let archive = b"archive bytes";
        let binary = b"binary bytes";
        let mut v: serde_json::Value =
            serde_json::from_str(&manifest_json("1.2.3", "x86_64-apple-darwin", archive)).unwrap();
        v["artifacts"][0]["sha256"] = sha256_hex(binary).into();
        let m = parse(v.to_string().as_bytes(), "v1.2.3").unwrap();
        assert!(verify_archive(archive, &m.artifacts[0]).is_err());
    }
}
