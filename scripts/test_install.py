#!/usr/bin/env python3
"""Tests for scripts/install.sh against a local fake release directory.

Run as: python3 scripts/test_install.py
No network: downloads use file:// URLs through XERJ_DOWNLOAD_BASE and
XERJ_RELEASES_URL. A stub `gh` stands in for the attestation check.
"""
import hashlib
import io
import json
import os
import platform
import shutil
import stat
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
INSTALL = HERE / "install.sh"
REPO = "unsigned-gg/xerj"
VERSION = "1.2.3"
TAG = "v1.2.3"
COMMIT = "0123456789abcdef0123456789abcdef01234567"

# Tools install.sh may use. A PATH built from only these proves the script has
# no hidden dependency, and lets a test remove `gh` without touching the host.
TOOLS = [
    "sh", "curl", "tar", "awk", "sed", "grep", "tr", "wc", "mkdir", "mv", "cp",
    "chmod", "rm", "uname", "mktemp", "cat", "head", "find", "dirname",
    "basename", "gzip", "cut", "sha256sum", "shasum", "ls", "id", "printf", "sort",
]


def host_target() -> str:
    arch = {"x86_64": "x86_64", "amd64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}[
        platform.machine().lower()
    ]
    if platform.system() == "Darwin":
        return arch + "-apple-darwin"
    return arch + "-unknown-linux-musl"


def fake_binary(version: str = VERSION) -> bytes:
    return ("#!/bin/sh\necho 'xerj v%s'\n" % version).encode()


class Release:
    """A fake release directory laid out like a GitHub release download."""

    def __init__(self, root: Path, version: str = VERSION, binary: bytes = None):
        self.version = version
        self.tag = "v" + version
        self.target = host_target()
        self.dir = root / "download" / self.tag
        self.dir.mkdir(parents=True, exist_ok=True)
        self.archive = "xerj-%s-%s.tar.gz" % (version, self.target)
        stage = "xerj-%s-%s" % (version, self.target)
        data = binary if binary is not None else fake_binary(version)
        with tarfile.open(self.dir / self.archive, "w:gz") as tf:
            info = tarfile.TarInfo(stage + "/xerj")
            info.size = len(data)
            info.mode = 0o755
            tf.addfile(info, io.BytesIO(data))
        self.write_sums()
        self.write_manifest()

    @property
    def digest(self) -> str:
        return hashlib.sha256((self.dir / self.archive).read_bytes()).hexdigest()

    def write_sums(self, digest: str = None, name: str = None):
        line = "%s  %s\n" % (digest or self.digest, name or self.archive)
        (self.dir / ("SHA256SUMS-" + self.target)).write_text(line)

    def write_manifest(self, sha256: str = None, size: int = None, tag: str = None):
        path = self.dir / self.archive
        artifacts = []
        for t in ("aarch64-apple-darwin", self.target, "x86_64-pc-windows-msvc"):
            name = self.archive if t == self.target else "xerj-%s-%s.tar.gz" % (self.version, t)
            artifacts.append({
                "archive": name,
                "binary": "xerj",
                "sha256": (sha256 or self.digest) if t == self.target else "0" * 64,
                "size": (size if size is not None else path.stat().st_size) if t == self.target else 1,
                "target": t,
                "url": "https://example.invalid/" + name,
            })
        manifest = {
            "package": "xerj", "channel": "stable", "version": self.version,
            "tag": tag or self.tag, "tag_commit": COMMIT, "artifacts": artifacts,
        }
        (self.dir / ("xerj-%s-manifest.json" % self.version)).write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n"
        )


class InstallTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.bin = self.root / "tools"
        self.bin.mkdir()
        for t in TOOLS:
            found = shutil.which(t)
            if found:
                (self.bin / t).symlink_to(found)
        self.dest = self.root / "dest"
        self.release = Release(self.root)

    def add_gh(self, exit_code: int):
        log = self.root / "gh.log"
        stub = self.bin / "gh"
        stub.write_text('#!/bin/sh\necho "$@" >> "%s"\nexit %d\n' % (log, exit_code))
        stub.chmod(0o755)
        return log

    def run_install(self, *args, env=None):
        e = {
            "PATH": str(self.bin),
            "HOME": str(self.root / "home"),
            "XERJ_DOWNLOAD_BASE": "file://" + str(self.root / "download"),
            "XERJ_INSTALL_DIR": str(self.dest),
        }
        e.update(env or {})
        return subprocess.run(
            [str(self.bin / "sh"), str(INSTALL), *args],
            env=e, capture_output=True, text=True, timeout=60,
        )

    def installed(self) -> Path:
        return self.dest / "xerj"


class HappyPathTests(InstallTest):
    def test_installs_the_verified_binary(self):
        r = self.run_install(TAG)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.installed().read_bytes(), fake_binary())
        self.assertTrue(os.access(self.installed(), os.X_OK))
        self.assertFalse((self.dest / "xerj.bak").exists())

    def test_existing_binary_is_kept_as_bak(self):
        self.dest.mkdir()
        self.installed().write_bytes(b"old-binary")
        r = self.run_install(TAG)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual((self.dest / "xerj.bak").read_bytes(), b"old-binary")
        self.assertEqual(self.installed().read_bytes(), fake_binary())

    def test_no_temp_files_left_in_dest(self):
        self.run_install(TAG)
        self.assertEqual(sorted(p.name for p in self.dest.iterdir()), ["xerj"])

    def test_latest_tag_skips_non_version_releases(self):
        releases = self.root / "releases.json"
        releases.write_text(json.dumps([
            {"tag_name": "pack-rust-vulns-2026-10-30"},
            {"tag_name": "v1.2.3"},
            {"tag_name": "v1.2.2"},
        ]))
        r = self.run_install(env={"XERJ_RELEASES_URL": "file://" + str(releases)})
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("v1.2.3", r.stdout)


class RefusalTests(InstallTest):
    def assert_refused(self, r, needle):
        self.assertNotEqual(r.returncode, 0, r.stdout)
        self.assertIn(needle, r.stdout + r.stderr)
        self.assertFalse(self.installed().exists(), "binary must not be installed")

    def test_tampered_archive_fails_sums(self):
        self.release.write_sums(digest="0" * 64)
        self.assert_refused(self.run_install(TAG), self.release.archive)

    def test_sums_without_a_line_for_the_archive(self):
        self.release.write_sums(name="xerj-1.2.3-other-target.tar.gz")
        self.assert_refused(self.run_install(TAG), "SHA256SUMS-" + self.release.target)

    def test_sums_file_missing(self):
        (self.release.dir / ("SHA256SUMS-" + self.release.target)).unlink()
        self.assert_refused(self.run_install(TAG), "SHA256SUMS-")

    def test_manifest_sha_mismatch(self):
        self.release.write_manifest(sha256="1" * 64)
        self.assert_refused(self.run_install(TAG), "manifest")

    def test_manifest_size_mismatch(self):
        self.release.write_manifest(size=1)
        self.assert_refused(self.run_install(TAG), "size")

    def test_manifest_for_another_tag(self):
        self.release.write_manifest(tag="v9.9.9")
        self.assert_refused(self.run_install(TAG), "manifest")

    def test_manifest_missing(self):
        (self.release.dir / "xerj-1.2.3-manifest.json").unlink()
        self.assert_refused(self.run_install(TAG), "manifest")

    def test_binary_reports_the_wrong_version(self):
        Release.__init__(self.release, self.root, binary=fake_binary("9.9.9"))
        self.dest.mkdir()
        self.installed().write_bytes(b"old-binary")
        r = self.run_install(TAG)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("--version", r.stdout + r.stderr)
        self.assertEqual(self.installed().read_bytes(), b"old-binary")
        self.assertFalse((self.dest / "xerj.bak").exists())

    def test_refusal_leaves_an_existing_binary_alone(self):
        self.dest.mkdir()
        self.installed().write_bytes(b"old-binary")
        self.release.write_sums(digest="0" * 64)
        self.assertNotEqual(self.run_install(TAG).returncode, 0)
        self.assertEqual(self.installed().read_bytes(), b"old-binary")

    def test_bad_tag_argument(self):
        r = self.run_install("main")
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)

    def test_unknown_flag(self):
        self.assertEqual(self.run_install("--nope").returncode, 2)


class AttestationTests(InstallTest):
    def test_gh_absent_warns_and_installs(self):
        r = self.run_install(TAG)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("provenance NOT verified", r.stdout + r.stderr)
        self.assertIn("gh", r.stdout + r.stderr)

    def test_gh_present_and_ok_verifies_against_the_fork(self):
        log = self.add_gh(0)
        r = self.run_install(TAG)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertNotIn("NOT verified", r.stdout + r.stderr)
        call = log.read_text()
        self.assertIn("attestation verify", call)
        self.assertIn("--repo " + REPO, call)
        self.assertIn(self.release.archive, call)

    def test_gh_present_and_failing_aborts(self):
        self.add_gh(1)
        self.dest.mkdir()
        self.installed().write_bytes(b"old-binary")
        r = self.run_install(TAG)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("attestation", r.stdout + r.stderr)
        self.assertEqual(self.installed().read_bytes(), b"old-binary")


class NoSourceBuildTests(unittest.TestCase):
    def test_script_never_builds_from_source(self):
        text = INSTALL.read_text()
        for word in ("cargo", "git clone", "rustc"):
            self.assertNotIn(word, text)

    def test_script_is_parsed_by_posix_sh(self):
        r = subprocess.run(["sh", "-n", str(INSTALL)], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)


if __name__ == "__main__":
    unittest.main()
