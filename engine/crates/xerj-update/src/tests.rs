//! End-to-end tests of `update` against a local HTTP fixture that imitates the
//! GitHub Releases API. No network, no real `gh`.

use super::*;
use crate::archive::tests::tar_gz;
use crate::manifest::tests::COMMIT;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

const TARGET: &str = "x86_64-unknown-linux-musl";

fn script(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho 'xerj v{version}'\n").into_bytes()
}

fn archive_of(version: &str, binary: &[u8]) -> Vec<u8> {
    let name = format!("xerj-{version}-{TARGET}/xerj");
    tar_gz(&[(name.as_str(), binary, tar::EntryType::Regular)])
}

type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;
type AssetList = Vec<(String, Vec<serde_json::Value>)>;

struct Server {
    base: String,
    /// (path, Authorization header if any)
    seen: Seen,
}

impl Server {
    fn hits(&self, prefix: &str) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.starts_with(prefix))
            .count()
    }
}

struct Rel<'a> {
    tag: &'a str,
    manifest: Option<Vec<u8>>,
    archive: Vec<u8>,
    /// Bytes served for the archive when they should differ from `archive`.
    served_archive: Option<Vec<u8>>,
}

fn rel(version: &str, binary: &[u8]) -> Rel<'static> {
    let archive = archive_of(version, binary);
    let tag: &'static str = Box::leak(format!("v{version}").into_boxed_str());
    Rel {
        tag,
        manifest: Some(
            crate::manifest::tests::manifest_json(version, TARGET, &archive).into_bytes(),
        ),
        archive,
        served_archive: None,
    }
}

/// Routes for a repo `o/r` with the given releases, newest first.
fn serve(releases: Vec<Rel>) -> Server {
    let mut list =
        vec![serde_json::json!({"tag_name": "pack-rust-vulns-2026-10-30", "assets": []})];
    let mut next_id = 1;
    let mut assets_for: AssetList = Vec::new();
    let mut bodies: Vec<(String, Vec<u8>)> = Vec::new();
    for r in &releases {
        let mut assets = Vec::new();
        let version = r.tag.trim_start_matches('v');
        if let Some(m) = &r.manifest {
            let id = next_id;
            next_id += 1;
            assets.push((format!("xerj-{version}-manifest.json"), id, m.len()));
            bodies.push((format!("/repos/o/r/releases/assets/{id}"), m.clone()));
        }
        let id = next_id;
        next_id += 1;
        let served = r
            .served_archive
            .clone()
            .unwrap_or_else(|| r.archive.clone());
        assets.push((format!("xerj-{version}-{TARGET}.tar.gz"), id, served.len()));
        bodies.push((format!("/repos/o/r/releases/assets/{id}"), served));
        assets_for.push((
            r.tag.to_string(),
            assets.iter().map(|(n, i, s)| serde_json::json!({"name": n, "id": i, "size": s, "url": format!("@BASE@/repos/o/r/releases/assets/{i}")})).collect(),
        ));
        list.push(serde_json::json!({"tag_name": r.tag, "assets": []}));
    }
    build_server(list, assets_for, bodies)
}

fn build_server(
    list: Vec<serde_json::Value>,
    assets_for: AssetList,
    bodies: Vec<(String, Vec<u8>)>,
) -> Server {
    // Two-step: bind first to learn the address, then fill the routes the thread reads.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mut routes: HashMap<String, Vec<u8>> = HashMap::new();
    let fix = |v: &serde_json::Value| v.to_string().replace("@BASE@", &base).into_bytes();
    routes.insert(
        "/repos/o/r/releases?per_page=100".into(),
        fix(&serde_json::Value::Array(list)),
    );
    for (tag, assets) in assets_for {
        routes.insert(
            format!("/repos/o/r/releases/tags/{tag}"),
            fix(&serde_json::json!({"tag_name": tag, "assets": assets})),
        );
    }
    for (path, body) in bodies {
        routes.insert(path, body);
    }
    let seen: Seen = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let head = String::from_utf8_lossy(&buf).to_string();
            let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
            let auth = head.lines().find_map(|l| {
                l.to_ascii_lowercase()
                    .starts_with("authorization:")
                    .then(|| l.to_string())
            });
            log.lock().unwrap().push((path.clone(), auth));
            let (status, body) = match routes.get(&path) {
                Some(b) => ("200 OK", b.clone()),
                None => ("404 Not Found", b"{}".to_vec()),
            };
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(&body);
        }
    });
    Server { base, seen }
}

struct Env {
    _dir: tempfile::TempDir,
    exe: PathBuf,
    server: Server,
}

fn env(releases: Vec<Rel>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("xerj");
    std::fs::write(&exe, script("1.0.0-rc.100")).unwrap();
    std::fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    Env {
        _dir: dir,
        exe,
        server: serve(releases),
    }
}

impl Env {
    fn cfg(&self) -> Config {
        Config {
            repo: "o/r".into(),
            api_base: self.server.base.clone(),
            token: None,
            exe: self.exe.clone(),
            current_version: "1.0.0-rc.100".into(),
            target: TARGET.into(),
            gh_program: None,
        }
    }
    fn exe_bytes(&self) -> Vec<u8> {
        std::fs::read(&self.exe).unwrap()
    }
}

fn err(r: Result<Outcome>) -> String {
    format!("{:#}", r.unwrap_err())
}

#[test]
fn updates_to_the_newest_release_unauthenticated() {
    let e = env(vec![rel("1.0.0-rc.101", &script("1.0.0-rc.101"))]);
    let out = update(&e.cfg(), &Options::default()).unwrap();
    assert_eq!(
        out,
        Outcome::Updated {
            tag: "v1.0.0-rc.101".into()
        }
    );
    assert_eq!(e.exe_bytes(), script("1.0.0-rc.101"));
    assert_eq!(
        std::fs::read(swap::backup_path(&e.exe)).unwrap(),
        script("1.0.0-rc.100")
    );
    assert!(
        e.server
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|(_, a)| a.is_none()),
        "no token was configured"
    );
}

#[test]
fn a_configured_token_is_sent_to_the_api() {
    let e = env(vec![rel("1.0.0-rc.101", &script("1.0.0-rc.101"))]);
    let mut cfg = e.cfg();
    cfg.token = Some("test-token-value".into());
    update(&cfg, &Options::default()).unwrap();
    let seen = e.server.seen.lock().unwrap();
    assert!(!seen.is_empty());
    assert!(seen
        .iter()
        .all(|(_, a)| a.as_deref().map(|h| h.contains("Bearer ")).unwrap_or(false)));
}

#[test]
fn refuses_when_the_newest_release_is_not_newer() {
    let e = env(vec![rel("1.0.0-rc.100", &script("1.0.0-rc.100"))]);
    let out = update(&e.cfg(), &Options::default()).unwrap();
    assert_eq!(
        out,
        Outcome::Current {
            latest: "v1.0.0-rc.100".into()
        }
    );
    assert_eq!(
        e.server.hits("/repos/o/r/releases/assets"),
        0,
        "nothing was downloaded"
    );
    assert_eq!(e.exe_bytes(), script("1.0.0-rc.100"));
}

#[test]
fn refuses_an_older_tag_by_name() {
    let e = env(vec![rel("1.0.0-rc.99", &script("1.0.0-rc.99"))]);
    let msg = err(update(
        &e.cfg(),
        &Options {
            tag: Some("v1.0.0-rc.99".into()),
            ..Options::default()
        },
    ));
    assert!(msg.contains("not newer"), "{msg}");
    assert_eq!(e.exe_bytes(), script("1.0.0-rc.100"));
}

#[test]
fn check_reports_and_installs_nothing() {
    let e = env(vec![rel("1.0.0-rc.101", &script("1.0.0-rc.101"))]);
    let out = update(
        &e.cfg(),
        &Options {
            check: true,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(
        out,
        Outcome::Available {
            tag: "v1.0.0-rc.101".into()
        }
    );
    assert_eq!(e.exe_bytes(), script("1.0.0-rc.100"));
    assert!(!swap::backup_path(&e.exe).exists());
}

#[test]
fn a_tampered_archive_is_refused_and_named() {
    let mut r = rel("1.0.0-rc.101", &script("1.0.0-rc.101"));
    let mut bad = r.archive.clone();
    let last = bad.len() - 1;
    bad[last] ^= 0xff; // same size, different bytes
    r.served_archive = Some(bad);
    let e = env(vec![r]);
    let msg = err(update(&e.cfg(), &Options::default()));
    assert!(
        msg.contains("xerj-1.0.0-rc.101-x86_64-unknown-linux-musl.tar.gz")
            && msg.contains("sha256"),
        "{msg}"
    );
    assert_eq!(e.exe_bytes(), script("1.0.0-rc.100"));
}

#[test]
fn a_short_archive_fails_the_size_check() {
    let mut r = rel("1.0.0-rc.101", &script("1.0.0-rc.101"));
    r.served_archive = Some(r.archive[..r.archive.len() - 3].to_vec());
    let e = env(vec![r]);
    let msg = err(update(&e.cfg(), &Options::default()));
    assert!(msg.contains("bytes"), "{msg}");
    assert_eq!(e.exe_bytes(), script("1.0.0-rc.100"));
}

#[test]
fn a_manifest_that_hashes_the_binary_is_refused() {
    let mut r = rel("1.0.0-rc.101", &script("1.0.0-rc.101"));
    let mut m: serde_json::Value = serde_json::from_slice(r.manifest.as_ref().unwrap()).unwrap();
    m["artifacts"][0]["sha256"] = crate::manifest::sha256_hex(&script("1.0.0-rc.101")).into();
    r.manifest = Some(m.to_string().into_bytes());
    let e = env(vec![r]);
    assert!(err(update(&e.cfg(), &Options::default())).contains("sha256"));
    assert_eq!(e.exe_bytes(), script("1.0.0-rc.100"));
}

#[test]
fn a_release_without_a_manifest_is_refused() {
    let mut r = rel("1.0.0-rc.101", &script("1.0.0-rc.101"));
    r.manifest = None;
    let e = env(vec![r]);
    assert!(err(update(&e.cfg(), &Options::default())).contains("predates verified releases"));
}

#[test]
fn a_target_missing_from_the_manifest_is_refused() {
    let e = env(vec![rel("1.0.0-rc.101", &script("1.0.0-rc.101"))]);
    let mut cfg = e.cfg();
    cfg.target = "aarch64-apple-darwin".into();
    assert!(
        err(update(&cfg, &Options::default())).contains("no entry for target aarch64-apple-darwin")
    );
}

#[test]
fn a_binary_with_the_wrong_version_is_refused() {
    let e = env(vec![rel("1.0.0-rc.101", &script("9.9.9"))]);
    let msg = err(update(&e.cfg(), &Options::default()));
    assert!(msg.contains("--version"), "{msg}");
    assert_eq!(e.exe_bytes(), script("1.0.0-rc.100"));
    assert!(!swap::backup_path(&e.exe).exists());
}

#[test]
fn an_unknown_tag_is_an_error() {
    let e = env(vec![rel("1.0.0-rc.101", &script("1.0.0-rc.101"))]);
    let msg = err(update(
        &e.cfg(),
        &Options {
            tag: Some("v1.0.0-rc.150".into()),
            ..Options::default()
        },
    ));
    assert!(msg.contains("404"), "{msg}");
}

fn gh_stub(dir: &Path, code: i32) -> String {
    let p = dir.join("gh-stub");
    std::fs::write(&p, format!("#!/bin/sh\nexit {code}\n")).unwrap();
    std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    p.to_string_lossy().into()
}

#[test]
fn a_failing_attestation_aborts_before_the_swap() {
    let e = env(vec![rel("1.0.0-rc.101", &script("1.0.0-rc.101"))]);
    let mut cfg = e.cfg();
    cfg.gh_program = Some(gh_stub(e.exe.parent().unwrap(), 1));
    assert!(err(update(&cfg, &Options::default())).contains("attestation"));
    assert_eq!(e.exe_bytes(), script("1.0.0-rc.100"));
}

#[test]
fn a_passing_attestation_lets_the_update_through() {
    let e = env(vec![rel("1.0.0-rc.101", &script("1.0.0-rc.101"))]);
    let mut cfg = e.cfg();
    cfg.gh_program = Some(gh_stub(e.exe.parent().unwrap(), 0));
    update(&cfg, &Options::default()).unwrap();
    assert_eq!(e.exe_bytes(), script("1.0.0-rc.101"));
}

#[test]
fn rollback_puts_the_previous_binary_back() {
    let e = env(vec![rel("1.0.0-rc.101", &script("1.0.0-rc.101"))]);
    update(&e.cfg(), &Options::default()).unwrap();
    let out = update(
        &e.cfg(),
        &Options {
            rollback: true,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(
        out,
        Outcome::RolledBack {
            reported: "xerj v1.0.0-rc.100".into()
        }
    );
    assert_eq!(e.exe_bytes(), script("1.0.0-rc.100"));
}

#[test]
fn argument_parsing() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert!(parse_args(&s(&["--check"])).unwrap().unwrap().check);
    assert_eq!(
        parse_args(&s(&["--tag", "v1.2.3"]))
            .unwrap()
            .unwrap()
            .tag
            .as_deref(),
        Some("v1.2.3")
    );
    assert!(parse_args(&s(&["--help"])).unwrap().is_none());
    assert!(parse_args(&s(&["--tag"])).is_err());
    assert!(parse_args(&s(&["--nope"])).is_err());
    assert!(parse_args(&s(&["--rollback", "--check"])).is_err());
}

#[test]
fn tag_commit_const_is_a_full_sha() {
    assert_eq!(COMMIT.len(), 40);
}
