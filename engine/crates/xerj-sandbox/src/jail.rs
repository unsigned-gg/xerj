use std::cell::Cell;
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::rules::AccessRules;

#[derive(Debug, Error)]
pub enum JailError {
    #[error("Landlock restriction failed: {0}")]
    Landlock(String),
    #[error("Seatbelt restriction failed: {0}")]
    Seatbelt(String),
    #[error("Path resolution error for {path}: {source}")]
    PathError {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Platform `{0}` has no supported jail backend (only linux/Landlock and macos/Seatbelt are implemented)")]
    Unsupported(&'static str),
}

// Per-OS-thread marker set by `Jail::enter` when kernel enforcement actually
// took effect (`enforced == true`).
//
// Why `thread_local!` and NOT an `AtomicBool`: Landlock's `restrict_self()` is
// a `prctl` + `landlock_restrict_self(2)` on the CALLING OS thread; the parent
// process's other threads (a Tokio runtime pool, test-runner worker threads)
// do NOT inherit the ruleset — only children `fork`/`clone`d from the calling
// thread after `enter()` do. A process-wide flag would falsely report an
// un-entered sibling thread as jailed if `tokio::spawn` moved the task across
// worker threads between `Jail::enter` and `Command::spawn`.
thread_local! {
    static JAILED_HERE: Cell<bool> = const { Cell::new(false) };
}

/// Returns `true` iff [`Jail::enter`] has succeeded with `enforced() == true`
/// on **this** OS thread.
///
/// Because `Jail::enter` restricts the calling thread and its future child
/// processes, calling [`std::process::Command::spawn`] on the same thread that
/// called `enter` guarantees the child inherits the restriction (Linux
/// Landlock) or is wrapped with `command_prefix` (macOS Seatbelt). Calling
/// `spawn` on a *different* thread (e.g. after a cross-thread `tokio::spawn`)
/// escapes Landlock on Linux — check this function at the spawn site to catch
/// that bug in tests and debug builds.
pub fn current_thread_is_jailed() -> bool {
    JAILED_HERE.with(Cell::get)
}

/// Name of the active OS jail backend compiled for this target.
pub const JAIL_MECHANISM: &str = if cfg!(target_os = "linux") {
    "landlock"
} else if cfg!(target_os = "macos") {
    "seatbelt"
} else {
    "unsupported"
};

/// Landlock ABI version targeted by [`Jail::enter`] on Linux.
///
/// Kept as a single source of truth so callers (e.g. containment-evidence
/// emission in [`crate::evidence`]) report the exact ABI constant the ruleset
/// was built against rather than open-coding a number that could drift if the
/// target ABI is bumped. On non-Linux targets the value is `0`.
#[cfg(target_os = "linux")]
pub const LANDLOCK_ABI: u32 = 2;

#[cfg(not(target_os = "linux"))]
pub const LANDLOCK_ABI: u32 = 0;

/// Bitmask of `LANDLOCK_ACCESS_FS_*` flags handled by the ruleset that
/// [`Jail::enter`] installs on Linux (`AccessFs::from_all(ABI::V2)`).
///
/// Computed directly from `landlock::AccessFs::from_all(ABI::V2)` so the
/// reported mask in [`crate::evidence`] is the exact `handled_access_fs` word
/// passed to `landlock_create_ruleset(2)`. Returns `0` on non-Linux targets.
#[cfg(target_os = "linux")]
pub fn handled_access_mask() -> u64 {
    use landlock::{Access, AccessFs, BitFlags, ABI};
    let flags: BitFlags<AccessFs> = AccessFs::from_all(ABI::V2);
    flags.bits()
}

#[cfg(not(target_os = "linux"))]
pub fn handled_access_mask() -> u64 {
    0
}

/// Compute the read-write path list handed to [`JailBuilder`] for a workspace
/// and optional store directory.
///
/// Why three roots instead of just `workspace`:
/// 1. `workspace` — the working directory where the jailed command writes.
/// 2. `store_root.parent()` (or `store_root`) — when the workspace is a
///    copy-on-write clone or refnlink tree under a store root, tools may write
///    sibling lockfiles or temp directories inside the store parent.
/// 3. [`std::env::temp_dir()`] — compilers (`rustc`, `cc`, `ld`), shells
///    (heredocs), and test harnesses unconditionally create scratch files in
///    `$TMPDIR` / `/tmp`. `enter_linux` only grants `/usr`, `/lib*`, `/bin`,
///    `/sbin`, `/etc`, `/dev`, `/proc` read-only; without `temp_dir()` as a
///    write root, any build command under Landlock fails with `EACCES` on its
///    first temp file.
pub fn jail_write_roots(workspace: &Path, store_root: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = vec![workspace.to_path_buf()];
    if let Some(sr) = store_root {
        let store_grant = sr
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(sr)
            .to_path_buf();
        if !roots.contains(&store_grant) {
            roots.push(store_grant);
        }
    }
    let tmp = std::env::temp_dir();
    if !roots.contains(&tmp) {
        roots.push(tmp);
    }
    roots
}

/// Builder for configuring and entering a process jail.
pub struct JailBuilder {
    rules: AccessRules,
}

impl JailBuilder {
    pub fn new(rules: AccessRules) -> Self {
        Self { rules }
    }

    /// Consumes the builder and applies OS-level restrictions to the current
    /// thread/process (Linux Landlock) or prepares the wrapper profile (macOS
    /// Seatbelt).
    pub fn enter(self) -> Result<Jail, JailError> {
        Jail::enter(self.rules)
    }
}

/// Active jail handle returned by [`JailBuilder::enter`] or [`Jail::enter`].
#[derive(Debug)]
pub struct Jail {
    rules: AccessRules,
    enforced: bool,
    seatbelt_profile: Option<String>,
}

impl Jail {
    /// Applies OS-level restrictions for `rules`.
    ///
    /// - **Linux:** Installs a Landlock V2 ruleset on the calling thread via
    ///   `restrict_self()`. System runtime paths (`/usr`, `/lib`, `/lib64`,
    ///   `/bin`, `/sbin`, `/etc`, `/dev`, `/proc`) receive read-only access;
    ///   `rules.read_paths` receive read+execute; `rules.write_paths` receive
    ///   full read+write+execute.
    /// - **macOS:** Generates a Seatbelt (`sandbox-exec`) S-expression profile
    ///   that denies network (except `localhost`) and denies `file-write*`
    ///   outside `rules.write_paths` and `/dev/null` / `/dev/dtracehelper`.
    ///   Use [`Jail::command_prefix`] when spawning child processes.
    /// - **Other OS:** Returns [`JailError::Unsupported`] (fail-closed).
    pub fn enter(rules: AccessRules) -> Result<Self, JailError> {
        #[cfg(target_os = "linux")]
        {
            Self::enter_linux(rules)
        }
        #[cfg(target_os = "macos")]
        {
            Self::enter_macos(rules)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = rules;
            Err(JailError::Unsupported(std::env::consts::OS))
        }
    }

    /// Whether the kernel-level restriction is actively enforced.
    ///
    /// On Linux this is `true` when the running kernel supports Landlock ABI V2
    /// and `RulesetStatus` is `FullyEnforced` or `PartiallyEnforced`, and
    /// `false` if the host kernel lacks Landlock (`RulesetStatus::NotEnforced`).
    pub fn enforced(&self) -> bool {
        self.enforced
    }

    /// The access rules this jail was configured with.
    pub fn rules(&self) -> &AccessRules {
        &self.rules
    }

    /// Name of the active backend (`"landlock"` or `"seatbelt"`).
    pub fn mechanism(&self) -> &'static str {
        JAIL_MECHANISM
    }

    /// Returns a command wrapper prefix if the backend requires external process
    /// wrapping (`["sandbox-exec", "-p", "<profile>"]` on macOS, empty on Linux
    /// because Landlock is inherited across `fork`/`execve`).
    pub fn command_prefix(&self) -> Vec<String> {
        if let Some(ref profile) = self.seatbelt_profile {
            vec![
                "sandbox-exec".to_string(),
                "-p".to_string(),
                profile.clone(),
            ]
        } else {
            Vec::new()
        }
    }

    #[cfg(target_os = "linux")]
    fn enter_linux(rules: AccessRules) -> Result<Self, JailError> {
        use landlock::{
            Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
            RulesetStatus, ABI,
        };

        let abi = ABI::V2;
        let read_only = AccessFs::Execute | AccessFs::ReadFile | AccessFs::ReadDir;
        let read_write = AccessFs::from_all(abi);

        let ruleset = Ruleset::default()
            .handle_access(AccessFs::from_all(abi))
            .map_err(|e| JailError::Landlock(e.to_string()))?;

        let mut ruleset_created = ruleset
            .create()
            .map_err(|e| JailError::Landlock(e.to_string()))?;

        // Essential read-only system paths required for dynamic linking, DNS,
        // and basic process execution. `/tmp` is intentionally NOT in this list
        // so arbitrary temp files are isolated unless explicitly granted.
        let sys_read_paths = [
            "/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc", "/dev", "/proc",
        ];
        for sys_path in sys_read_paths {
            let p = Path::new(sys_path);
            if p.exists() {
                if let Ok(fd) = PathFd::new(p) {
                    ruleset_created = ruleset_created
                        .add_rule(PathBeneath::new(fd, read_only))
                        .map_err(|e| JailError::Landlock(e.to_string()))?;
                }
            }
        }

        // User-specified read-only paths — MUST exist and be resolvable.
        for path in &rules.read_paths {
            let canonical = std::fs::canonicalize(path).map_err(|source| JailError::PathError {
                path: path.clone(),
                source,
            })?;
            let fd = PathFd::new(&canonical).map_err(|e| JailError::PathError {
                path: canonical.clone(),
                source: std::io::Error::other(e.to_string()),
            })?;
            ruleset_created = ruleset_created
                .add_rule(PathBeneath::new(fd, read_only))
                .map_err(|e| JailError::Landlock(e.to_string()))?;
        }

        // User-specified read-write paths — MUST exist and be resolvable.
        for path in &rules.write_paths {
            let canonical = std::fs::canonicalize(path).map_err(|source| JailError::PathError {
                path: path.clone(),
                source,
            })?;
            let fd = PathFd::new(&canonical).map_err(|e| JailError::PathError {
                path: canonical.clone(),
                source: std::io::Error::other(e.to_string()),
            })?;
            ruleset_created = ruleset_created
                .add_rule(PathBeneath::new(fd, read_write))
                .map_err(|e| JailError::Landlock(e.to_string()))?;
        }

        let status = ruleset_created
            .restrict_self()
            .map_err(|e| JailError::Landlock(e.to_string()))?;

        let enforced = matches!(
            status.ruleset,
            RulesetStatus::FullyEnforced | RulesetStatus::PartiallyEnforced
        );

        if enforced {
            JAILED_HERE.with(|flag| flag.set(true));
        }

        Ok(Self {
            rules,
            enforced,
            seatbelt_profile: None,
        })
    }

    #[cfg(target_os = "macos")]
    fn enter_macos(rules: AccessRules) -> Result<Self, JailError> {
        let profile = build_seatbelt_profile(&rules)?;
        JAILED_HERE.with(|flag| flag.set(true));
        Ok(Self {
            rules,
            enforced: true,
            seatbelt_profile: Some(profile),
        })
    }
}

/// System paths granted read-only under the macOS Seatbelt profile so that
/// dynamic linking (`dyld`), system frameworks, and standard POSIX utilities
/// can execute while everything else (`/Users`, `/Volumes`, ungranted `/tmp`
/// subtrees, etc.) remains denied for both reads and writes.
///
/// Mirrors the Linux Landlock `sys_read_paths` list in `Jail::enter_linux`,
/// adapted for Darwin's filesystem layout (where `/etc`, `/var`, `/tmp` live
/// under `/private` and `/System`, `/Library` hold frameworks).
pub const MACOS_SYSTEM_READ_PATHS: &[&str] = &[
    "/bin",
    "/sbin",
    "/usr",
    "/lib",
    "/System",
    "/Library",
    "/dev",
    "/private/etc",
    "/private/var/db",
    "/private/var/select",
    "/var",
    "/etc",
];

/// Escape a path string for inclusion inside a Seatbelt S-expression string
/// literal (`"..."`).
///
/// Seatbelt profiles are Scheme-dialect S-expressions parsed by `libsandbox`.
/// Without escaping, a workspace path containing `"` or `\` (valid on both APFS
/// and ext4) breaks out of `(subpath "...")` and injects arbitrary Seatbelt
/// directives such as `(allow default)`. Rejecting NUL and escaping `\` and `"`
/// closes that injection vector.
pub fn escape_seatbelt_path(raw: &str) -> Result<String, JailError> {
    if raw.contains('\0') {
        return Err(JailError::Seatbelt(
            "path contains NUL byte, which is invalid in Seatbelt profiles".to_string(),
        ));
    }
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    Ok(out)
}

/// Builds a macOS Seatbelt (`sandbox-exec`) profile string from [`AccessRules`].
///
/// Exposed on all platforms so unit tests can verify profile generation on
/// Linux CI as well as macOS hosts.
///
/// Security model (mirrors Linux Landlock):
/// - `(deny file*)` blocks **both** `file-read*` and `file-write*` outside the
///   explicitly allowed subpaths, plus `(allow file-read-metadata)` at root `/`
///   so `dyld` and `realpath(3)` can traverse directory components.
/// - System runtime paths ([`MACOS_SYSTEM_READ_PATHS`]) and `rules.read_paths`
///   receive `file-read*`.
/// - `rules.write_paths` receive both `file-read*` and `file-write*`.
/// - All paths are canonicalized and escaped via [`escape_seatbelt_path`].
pub fn build_seatbelt_profile(rules: &AccessRules) -> Result<String, JailError> {
    let mut profile = String::from("(version 1)\n(allow default)\n");

    // Network policy: deny all network, then optionally re-allow localhost.
    profile.push_str("(deny network*)\n");
    if rules.allow_localhost_tcp {
        profile.push_str("(allow network* (remote ip \"localhost:*\"))\n");
        profile.push_str("(allow network* (local ip \"localhost:*\"))\n");
    }

    // Filesystem policy: deny all file reads AND writes by default, then
    // re-allow root metadata, system runtime reads, user read_paths, and user
    // write_paths. Using `(deny file*)` instead of `(deny file-write*)` ensures
    // `rules.read_paths` is enforced on macOS just like on Linux Landlock.
    profile.push_str("(deny file*)\n");
    profile.push_str("(allow file-read-metadata (literal \"/\"))\n");
    profile.push_str(
        "(allow file-read* file-write* (literal \"/dev/null\") (literal \"/dev/dtracehelper\") (literal \"/dev/tty\"))\n",
    );

    for sys in MACOS_SYSTEM_READ_PATHS {
        let escaped = escape_seatbelt_path(sys)?;
        profile.push_str(&format!("(allow file-read* (subpath \"{escaped}\"))\n"));
    }

    for path in &rules.read_paths {
        let canonical = std::fs::canonicalize(path).map_err(|source| JailError::PathError {
            path: path.clone(),
            source,
        })?;
        let escaped = escape_seatbelt_path(&canonical.to_string_lossy())?;
        profile.push_str(&format!("(allow file-read* (subpath \"{escaped}\"))\n"));
    }

    for path in &rules.write_paths {
        let canonical = std::fs::canonicalize(path).map_err(|source| JailError::PathError {
            path: path.clone(),
            source,
        })?;
        let escaped = escape_seatbelt_path(&canonical.to_string_lossy())?;
        profile.push_str(&format!(
            "(allow file-read* file-write* (subpath \"{escaped}\"))\n"
        ));
    }

    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn seatbelt_profile_denies_all_files_and_whitelists_reads_and_writes() {
        let ro_dir = TempDir::new().unwrap();
        let rw_dir = TempDir::new().unwrap();
        let rules = AccessRules::default()
            .with_read(ro_dir.path())
            .with_write(rw_dir.path());

        let profile = build_seatbelt_profile(&rules).unwrap();
        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny network*)"));
        assert!(profile.contains("(allow network* (remote ip \"localhost:*\"))"));
        assert!(profile.contains("(deny file*)"));
        assert!(profile.contains("(allow file-read* (subpath \"/usr\"))"));

        let ro_canon = std::fs::canonicalize(ro_dir.path()).unwrap();
        let rw_canon = std::fs::canonicalize(rw_dir.path()).unwrap();
        assert!(
            profile.contains(&format!(
                "(allow file-read* (subpath \"{}\"))",
                ro_canon.display()
            )),
            "read_paths must be emitted as file-read* subpaths in Seatbelt profile:\n{profile}"
        );
        assert!(
            profile.contains(&format!(
                "(allow file-read* file-write* (subpath \"{}\"))",
                rw_canon.display()
            )),
            "write_paths must be emitted as file-read* file-write* subpaths:\n{profile}"
        );
    }

    #[test]
    fn seatbelt_profile_omits_localhost_when_disabled() {
        let rules = AccessRules {
            read_paths: vec![],
            write_paths: vec![],
            allow_localhost_tcp: false,
        };
        let profile = build_seatbelt_profile(&rules).unwrap();
        assert!(profile.contains("(deny network*)"));
        assert!(!profile.contains("localhost:*"));
    }

    #[test]
    fn seatbelt_escape_neutralizes_quotes_and_backslashes_and_rejects_nul() {
        let injected = "/tmp/evil\"))(allow default)(comment \"";
        let escaped = escape_seatbelt_path(injected).unwrap();
        assert_eq!(escaped, r#"/tmp/evil\"))(allow default)(comment \""#);

        let with_backslash = r"/tmp/a\b";
        assert_eq!(escape_seatbelt_path(with_backslash).unwrap(), r"/tmp/a\\b");

        let with_nul = "/tmp/bad\0path";
        assert!(matches!(
            escape_seatbelt_path(with_nul),
            Err(JailError::Seatbelt(_))
        ));
    }

    #[test]
    fn enter_fails_on_nonexistent_write_path() {
        let rules = AccessRules::default().with_write("/nonexistent/xerj/jail/path/xyz987");
        let res = JailBuilder::new(rules).enter();
        assert!(
            matches!(res, Err(JailError::PathError { .. })),
            "expected PathError on missing write_path, got: {res:?}"
        );
    }

    #[test]
    fn enter_fails_on_nonexistent_read_path() {
        let rules = AccessRules::default().with_read("/nonexistent/xerj/jail/read/xyz987");
        let res = JailBuilder::new(rules).enter();
        assert!(
            matches!(res, Err(JailError::PathError { .. })),
            "expected PathError on missing read_path, got: {res:?}"
        );
    }

    #[test]
    fn jail_write_roots_includes_workspace_store_parent_and_temp_dir() {
        let ws = Path::new("/tmp/ws-1");
        let store = Path::new("/var/lib/xerj/store");
        let roots = jail_write_roots(ws, Some(store));
        assert!(roots.contains(&PathBuf::from("/tmp/ws-1")));
        assert!(roots.contains(&PathBuf::from("/var/lib/xerj")));
        assert!(roots.contains(&std::env::temp_dir()));
    }

    /// Runs `Jail::enter` inside a dedicated OS thread (because `restrict_self`
    /// permanently restricts the calling thread) and verifies that:
    /// 1. Writes inside the allowed `write_paths` workspace succeed.
    /// 2. Writes to a sibling directory outside `write_paths` are blocked by
    ///    the kernel when Landlock is enforced on the host.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_landlock_blocks_writes_outside_allowed_workspace() {
        let allowed_dir = TempDir::new().unwrap();
        let forbidden_dir = TempDir::new().unwrap();

        let allowed_path = allowed_dir.path().to_path_buf();
        let forbidden_path = forbidden_dir.path().to_path_buf();

        assert!(
            !current_thread_is_jailed(),
            "test runner thread must start unjailed"
        );

        let handle = std::thread::spawn(move || {
            assert!(
                !current_thread_is_jailed(),
                "fresh worker thread must start unjailed before Jail::enter"
            );
            let rules = AccessRules::default().with_write(&allowed_path);
            let jail = JailBuilder::new(rules).enter().expect("Jail::enter failed");
            assert_eq!(jail.mechanism(), "landlock");
            assert!(jail.command_prefix().is_empty());

            let ok_file = allowed_path.join("inside.txt");
            std::fs::write(&ok_file, b"allowed write")
                .expect("write inside allowed workspace must succeed");
            assert_eq!(std::fs::read(&ok_file).unwrap(), b"allowed write");

            let blocked_file = forbidden_path.join("escape.txt");
            let outside_write = std::fs::write(&blocked_file, b"should fail");

            if jail.enforced() {
                assert!(
                    current_thread_is_jailed(),
                    "current_thread_is_jailed() must be true on the thread that called Jail::enter"
                );
                let err = outside_write.expect_err(
                    "Landlock is enforced on this kernel, so writing outside write_paths MUST fail",
                );
                assert_eq!(
                    err.kind(),
                    std::io::ErrorKind::PermissionDenied,
                    "expected PermissionDenied (EACCES) from Landlock, got: {err:?}"
                );
            }
        });

        handle.join().expect("jailed worker thread panicked");
        assert!(
            !current_thread_is_jailed(),
            "parent thread must NOT observe child thread's JAILED_HERE flag"
        );
    }
}
