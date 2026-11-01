//! Replace the running binary: lock, stage, check `--version`, keep a `.bak`,
//! then rename into place. Rename is atomic on one filesystem, so the installed
//! binary is either the old one or the new one, never a partial file.

use anyhow::{bail, Context, Result};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Held for the whole update. The operating system releases it when the holder's
/// file descriptor closes, so a killed update never leaves a stale lock.
pub struct UpdateLock {
    _file: File,
}

impl UpdateLock {
    pub fn acquire(dir: &Path) -> Result<UpdateLock> {
        let path = dir.join(".xerj-update.lock");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| {
                format!(
                    "cannot open the lock file {} (is the directory writable?)",
                    path.display()
                )
            })?;
        match file.try_lock() {
            Ok(()) => {
                file.set_len(0).ok();
                let _ = write!(file, "{}", std::process::id());
                Ok(UpdateLock { _file: file })
            }
            Err(TryLockError::WouldBlock) => {
                let holder = fs::read_to_string(&path).unwrap_or_default();
                bail!(
                    "another xerj update is running (pid {}); lock {}",
                    holder.trim(),
                    path.display()
                )
            }
            Err(TryLockError::Error(e)) => Err(e).context("cannot take the update lock"),
        }
    }
}

pub fn backup_path(exe: &Path) -> PathBuf {
    let mut name = exe
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".bak");
    exe.with_file_name(name)
}

/// `--version` must print `xerj v<version>` (first two words), so a companion or
/// look-alike binary is never installed as xerj.
pub fn version_output_matches(output: &str, version: &str) -> bool {
    let mut words = output.lines().next().unwrap_or("").split_whitespace();
    words.next() == Some("xerj") && words.next() == Some(&format!("v{version}"))
}

fn run_version(bin: &Path) -> Result<String> {
    // A binary written a moment ago can briefly report ETXTBSY if another thread
    // forked while its descriptor was open; retry a few times.
    let mut last = None;
    for _ in 0..20 {
        match Command::new(bin)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
        {
            Ok(out) => return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
            Err(e) if e.raw_os_error() == Some(26) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                return Err(e).with_context(|| format!("cannot run {} --version", bin.display()))
            }
        }
    }
    Err(last.unwrap()).context("cannot run the staged binary")
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .context("cannot chmod the staged binary")
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<()> {
    Ok(())
}

fn stage(dir: &Path, bytes: Option<&[u8]>, copy_from: Option<&Path>) -> Result<tempfile::TempPath> {
    let mut tmp = tempfile::Builder::new()
        .prefix(".xerj-stage-")
        .tempfile_in(dir)
        .with_context(|| format!("cannot stage a file in {}", dir.display()))?;
    if let Some(b) = bytes {
        tmp.write_all(b)?;
    }
    if let Some(src) = copy_from {
        let mut input =
            File::open(src).with_context(|| format!("cannot read {}", src.display()))?;
        std::io::copy(&mut input, &mut tmp)?;
    }
    tmp.flush()?;
    let path = tmp.into_temp_path(); // closes the handle, keeps the auto-delete
    make_executable(&path)?;
    Ok(path)
}

fn dir_of(exe: &Path) -> Result<&Path> {
    exe.parent().context("the executable path has no directory")
}

/// Install `new_binary` over `exe`, keeping the previous binary as `<exe>.bak`.
/// On any failure before the final rename, `exe` is untouched.
pub fn install(exe: &Path, new_binary: &[u8], version: &str) -> Result<()> {
    if !cfg!(unix) {
        bail!("xerj update is supported on Linux and macOS only");
    }
    let dir = dir_of(exe)?;
    let _lock = UpdateLock::acquire(dir)?;
    let staged = stage(dir, Some(new_binary), None)?;
    let reported = run_version(&staged)?;
    if !version_output_matches(&reported, version) {
        bail!("the new binary's --version says '{reported}', expected 'xerj v{version}'; nothing was installed");
    }
    if exe.exists() {
        let keep = stage(dir, None, Some(exe))?;
        keep.persist(backup_path(exe))
            .map_err(|e| e.error)
            .context("cannot write the .bak")?;
    }
    staged
        .persist(exe)
        .map_err(|e| e.error)
        .context("cannot move the new binary into place")?;
    Ok(())
}

/// Put `<exe>.bak` back as `exe`. The `.bak` stays, so a rollback can be repeated.
pub fn rollback(exe: &Path) -> Result<String> {
    if !cfg!(unix) {
        bail!("xerj update is supported on Linux and macOS only");
    }
    let dir = dir_of(exe)?;
    let bak = backup_path(exe);
    if !bak.is_file() {
        bail!("no backup at {}; nothing to roll back to", bak.display());
    }
    let _lock = UpdateLock::acquire(dir)?;
    let staged = stage(dir, None, Some(&bak))?;
    let reported = run_version(&staged)?;
    if reported.split_whitespace().next() != Some("xerj") {
        bail!("the backup does not report an xerj version ('{reported}'); left in place");
    }
    staged
        .persist(exe)
        .map_err(|e| e.error)
        .context("cannot move the backup into place")?;
    Ok(reported)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn script(version: &str) -> Vec<u8> {
        format!("#!/bin/sh\necho 'xerj v{version}'\n").into_bytes()
    }

    fn setup(old: Option<&[u8]>) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("xerj");
        if let Some(b) = old {
            fs::write(&exe, b).unwrap();
            make_executable(&exe).unwrap();
        }
        (dir, exe)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn version_prefix_is_a_word_match() {
        assert!(version_output_matches("xerj v1.2.3", "1.2.3"));
        assert!(version_output_matches("xerj v1.2.3 (abc)", "1.2.3"));
        assert!(!version_output_matches("xerj v1.2.3-rc.1", "1.2.3"));
        assert!(!version_output_matches("xerj-edge v1.2.3", "1.2.3"));
        assert!(!version_output_matches("", "1.2.3"));
    }

    #[test]
    fn install_swaps_and_keeps_a_bak() {
        let old = script("1.0.0");
        let (dir, exe) = setup(Some(&old));
        install(&exe, &script("1.2.3"), "1.2.3").unwrap();
        assert_eq!(fs::read(&exe).unwrap(), script("1.2.3"));
        assert_eq!(fs::read(backup_path(&exe)).unwrap(), old);
        assert_eq!(names(dir.path()), [".xerj-update.lock", "xerj", "xerj.bak"]);
    }

    #[test]
    fn install_without_an_existing_binary_makes_no_bak() {
        let (dir, exe) = setup(None);
        install(&exe, &script("1.2.3"), "1.2.3").unwrap();
        assert_eq!(names(dir.path()), [".xerj-update.lock", "xerj"]);
    }

    #[test]
    fn a_wrong_version_binary_installs_nothing() {
        let old = script("1.0.0");
        let (dir, exe) = setup(Some(&old));
        let e = install(&exe, &script("9.9.9"), "1.2.3")
            .unwrap_err()
            .to_string();
        assert!(e.contains("--version"), "{e}");
        assert_eq!(fs::read(&exe).unwrap(), old);
        assert_eq!(names(dir.path()), [".xerj-update.lock", "xerj"]);
    }

    #[test]
    fn a_binary_that_will_not_run_installs_nothing() {
        let old = script("1.0.0");
        let (_dir, exe) = setup(Some(&old));
        assert!(install(&exe, b"not an executable", "1.2.3").is_err());
        assert_eq!(fs::read(&exe).unwrap(), old);
    }

    #[test]
    fn a_held_lock_blocks_a_second_update() {
        let old = script("1.0.0");
        let (dir, exe) = setup(Some(&old));
        let _held = UpdateLock::acquire(dir.path()).unwrap();
        let e = install(&exe, &script("1.2.3"), "1.2.3")
            .unwrap_err()
            .to_string();
        assert!(e.contains("another xerj update is running"), "{e}");
        assert_eq!(fs::read(&exe).unwrap(), old);
    }

    #[test]
    fn the_lock_is_released_when_dropped() {
        let (dir, _exe) = setup(None);
        drop(UpdateLock::acquire(dir.path()).unwrap());
        UpdateLock::acquire(dir.path()).unwrap();
    }

    #[test]
    fn rollback_restores_the_bak_and_keeps_it() {
        let old = script("1.0.0");
        let (_dir, exe) = setup(Some(&old));
        install(&exe, &script("1.2.3"), "1.2.3").unwrap();
        let said = rollback(&exe).unwrap();
        assert_eq!(said, "xerj v1.0.0");
        assert_eq!(fs::read(&exe).unwrap(), old);
        assert_eq!(fs::read(backup_path(&exe)).unwrap(), old);
        rollback(&exe).unwrap(); // repeatable
    }

    #[test]
    fn rollback_without_a_bak_is_an_error() {
        let (_dir, exe) = setup(Some(&script("1.0.0")));
        let e = rollback(&exe).unwrap_err().to_string();
        assert!(e.contains("nothing to roll back"), "{e}");
    }
}
