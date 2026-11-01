//! `xerj update`: move the running binary to the newest release of the
//! unsigned-gg/xerj fork, or back to the previous one.
//!
//! The release manifest (`xerj-<version>-manifest.json`) is the contract: it
//! names the archive for each target with that archive's sha256 and byte size.
//! `xerj update` refuses anything that is not newer, any archive that differs
//! from the manifest, and any binary whose `--version` is not the release.
//! It checks provenance with `gh attestation verify` when `gh` is installed.
//!
//! What it does not do: the manifest and the archive come from the same release,
//! so matching them rules out damage and mix-ups, not a hostile release. The
//! attestation is what binds the archive to the fork's release workflow.

pub mod archive;
pub mod github;
pub mod manifest;
pub mod swap;
pub mod version;

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use version::Version;

pub const DEFAULT_REPO: &str = "unsigned-gg/xerj";
pub const DEFAULT_API_BASE: &str = "https://api.github.com";
const MAX_MANIFEST: u64 = 1 << 20;
const MAX_ARCHIVE: u64 = 512 << 20;

/// Where to update from and what to update. Tests build this directly.
pub struct Config {
    pub repo: String,
    pub api_base: String,
    pub token: Option<String>,
    pub exe: PathBuf,
    pub current_version: String,
    pub target: String,
    /// The `gh` program for the attestation check; `None` skips it with a warning.
    pub gh_program: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Options {
    pub check: bool,
    pub rollback: bool,
    pub tag: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing newer than the running binary.
    Current {
        latest: String,
    },
    /// `--check` found a newer release and installed nothing.
    Available {
        tag: String,
    },
    Updated {
        tag: String,
    },
    RolledBack {
        reported: String,
    },
}

/// The target triple this binary was built for, so an update stays on the same
/// libc flavour. `None` for a platform the fork does not release for.
pub fn host_target() -> Option<&'static str> {
    Some(
        if cfg!(all(
            target_os = "linux",
            target_arch = "x86_64",
            target_env = "musl"
        )) {
            "x86_64-unknown-linux-musl"
        } else if cfg!(all(
            target_os = "linux",
            target_arch = "x86_64",
            target_env = "gnu"
        )) {
            "x86_64-unknown-linux-gnu"
        } else if cfg!(all(
            target_os = "linux",
            target_arch = "aarch64",
            target_env = "musl"
        )) {
            "aarch64-unknown-linux-musl"
        } else if cfg!(all(
            target_os = "linux",
            target_arch = "aarch64",
            target_env = "gnu"
        )) {
            "aarch64-unknown-linux-gnu"
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            "x86_64-apple-darwin"
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "aarch64-apple-darwin"
        } else {
            return None;
        },
    )
}

fn check_provenance(cfg: &Config, archive_bytes: &[u8], archive_name: &str) -> Result<()> {
    let Some(gh) = &cfg.gh_program else {
        eprintln!(
            "warning: provenance NOT verified (gh is not available); only the manifest checks ran"
        );
        return Ok(());
    };
    let dir = tempfile::tempdir().context("cannot create a temp directory")?;
    let file = dir.path().join(archive_name);
    std::fs::write(&file, archive_bytes)?;
    let out = std::process::Command::new(gh)
        .args(["attestation", "verify"])
        .arg(&file)
        .args(["--repo", &cfg.repo])
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            println!("ok   {archive_name} has a provenance attestation from {}", cfg.repo);
            Ok(())
        }
        Ok(o) => bail!(
            "{archive_name}: attestation verification failed for {} (run 'gh auth login' if gh is not logged in): {}",
            cfg.repo,
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("warning: provenance NOT verified (gh is not installed); only the manifest checks ran");
            Ok(())
        }
        Err(e) => Err(e).context("cannot run gh attestation verify"),
    }
}

pub fn update(cfg: &Config, opts: &Options) -> Result<Outcome> {
    if opts.rollback {
        let reported = swap::rollback(&cfg.exe)?;
        return Ok(Outcome::RolledBack { reported });
    }
    if !version::valid_repo(&cfg.repo) {
        bail!("'{}' is not an <owner>/<repo> name", cfg.repo);
    }
    let current = Version::parse(&cfg.current_version).map_err(anyhow::Error::msg)?;
    let client = github::Client::new(&cfg.api_base, &cfg.repo, cfg.token.clone())?;
    let tag = match &opts.tag {
        Some(t) => {
            if !version::valid_tag(t) {
                bail!("'{t}' is not a vX.Y.Z tag");
            }
            t.clone()
        }
        None => client.latest_tag()?,
    };
    let candidate = Version::parse(&tag).map_err(anyhow::Error::msg)?;
    if candidate <= current {
        if opts.tag.is_some() && candidate < current {
            bail!("refusing to move from v{} to {tag}: not newer (use scripts/install.sh for a downgrade)", cfg.current_version);
        }
        return Ok(Outcome::Current { latest: tag });
    }
    let version = tag.trim_start_matches('v').to_string();

    let release = client.release(&tag)?;
    let m_name = manifest::manifest_name(&version);
    let m_asset = release
        .asset(&m_name)
        .with_context(|| format!("{tag} has no {m_name}; it predates verified releases"))?;
    let man = manifest::parse(&client.download(m_asset, MAX_MANIFEST)?, &tag)?;
    let art = man
        .artifact_for(&cfg.target)
        .with_context(|| format!("{m_name} has no entry for target {}", cfg.target))?;
    println!(
        "{tag} (commit {}) has {} for {}",
        &man.tag_commit[..12],
        art.archive,
        cfg.target
    );
    if opts.check {
        return Ok(Outcome::Available { tag });
    }

    let a_asset = release
        .asset(&art.archive)
        .with_context(|| format!("{tag} has no asset {}", art.archive))?;
    if art.size > MAX_ARCHIVE {
        bail!(
            "{}: manifest size {} is over the {MAX_ARCHIVE}-byte limit",
            art.archive,
            art.size
        );
    }
    let bytes = client.download(a_asset, art.size)?;
    manifest::verify_archive(&bytes, art)?;
    println!(
        "ok   {} matches the manifest (sha256 and size)",
        art.archive
    );
    check_provenance(cfg, &bytes, &art.archive)?;
    let binary = archive::extract_binary(&bytes, &version, &cfg.target)?;
    swap::install(&cfg.exe, &binary, &version)?;
    Ok(Outcome::Updated { tag })
}

const USAGE: &str = "usage: xerj update [--check] [--tag vX.Y.Z] [--rollback]

  (no flags)   install the newest vX.Y.Z release of the fork if it is newer
  --check      report what would be installed; install nothing
  --tag TAG    use this release instead of the newest
  --rollback   put the previous binary (xerj.bak) back

environment: XERJ_UPDATE_REPO (an <owner>/<repo>; default unsigned-gg/xerj, and
the provenance check is pinned to whichever repo this names), GITHUB_TOKEN or
GH_TOKEN (optional; the fork is public)";

fn parse_args(args: &[String]) -> Result<Option<Options>, String> {
    let mut o = Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Ok(None),
            "--check" => o.check = true,
            "--rollback" => o.rollback = true,
            "--tag" => o.tag = Some(it.next().ok_or("--tag needs a value")?.clone()),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if o.rollback && (o.check || o.tag.is_some()) {
        return Err("--rollback cannot be combined with --check or --tag".into());
    }
    Ok(Some(o))
}

/// Entry point for the `xerj update` subcommand. `args` are the words after it.
/// Returns the process exit code: 0 done or already current, 1 failure, 2 usage.
pub fn run_cli(current_version: &str, args: &[String]) -> i32 {
    let opts = match parse_args(args) {
        Ok(Some(o)) => o,
        Ok(None) => {
            println!("{USAGE}");
            return 0;
        }
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return 2;
        }
    };
    let Some(target) = host_target() else {
        eprintln!("error: no release is built for this platform");
        return 1;
    };
    let exe = match std::env::current_exe().and_then(|p| p.canonicalize()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot locate the running binary: {e}");
            return 1;
        }
    };
    let gh_present = std::process::Command::new("gh")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let cfg = Config {
        repo: std::env::var("XERJ_UPDATE_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string()),
        api_base: DEFAULT_API_BASE.to_string(),
        token: github::resolve_token(),
        exe,
        current_version: current_version.to_string(),
        target: target.to_string(),
        gh_program: gh_present.then(|| "gh".to_string()),
    };
    match update(&cfg, &opts) {
        Ok(Outcome::Current { latest }) => {
            println!("xerj v{current_version} is current (newest release: {latest})");
            0
        }
        Ok(Outcome::Available { tag }) => {
            println!("{tag} is newer than v{current_version}; run 'xerj update' to install it");
            0
        }
        Ok(Outcome::Updated { tag }) => {
            println!(
                "updated v{current_version} -> {tag}; the previous binary is kept as {}",
                swap::backup_path(&cfg.exe).display()
            );
            0
        }
        Ok(Outcome::RolledBack { reported }) => {
            println!("rolled back to {reported}");
            0
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
