//! Read the one binary out of a release `.tar.gz` without unpacking to disk.

use anyhow::{bail, Context, Result};
use std::io::Read;

/// Largest binary accepted from an archive.
const MAX_BINARY: u64 = 1 << 30;

/// The binary inside `xerj-<version>-<target>.tar.gz` is exactly
/// `xerj-<version>-<target>/xerj`. Any other entry is ignored, so a crafted
/// entry name can never be written anywhere: nothing is extracted by path.
pub fn extract_binary(archive: &[u8], version: &str, target: &str) -> Result<Vec<u8>> {
    let want = format!("xerj-{version}-{target}/xerj");
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().context("archive is not a readable tar.gz")? {
        let entry = entry.context("archive has a damaged entry")?;
        let path = entry.path().context("archive entry has no path")?;
        if path.to_str() != Some(want.as_str()) {
            continue;
        }
        if entry.header().entry_type() != tar::EntryType::Regular {
            bail!("{want} in the archive is not a regular file");
        }
        let mut out = Vec::new();
        entry
            .take(MAX_BINARY + 1)
            .read_to_end(&mut out)
            .context("could not read the binary")?;
        if out.is_empty() || out.len() as u64 > MAX_BINARY {
            bail!("{want} in the archive is empty or larger than {MAX_BINARY} bytes");
        }
        return Ok(out);
    }
    bail!("the archive has no {want}")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn tar_gz(entries: &[(&str, &[u8], tar::EntryType)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data, kind) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(*kind);
            h.set_size(if *kind == tar::EntryType::Regular {
                data.len() as u64
            } else {
                0
            });
            h.set_mode(0o755);
            if *kind == tar::EntryType::Symlink {
                h.set_link_name("/etc/passwd").unwrap();
            }
            // Write the name bytes directly so a '..' entry survives the builder.
            let bytes = h.as_old_mut();
            bytes.name[..name.len()].copy_from_slice(name.as_bytes());
            h.set_cksum();
            builder
                .append(
                    &h,
                    if *kind == tar::EntryType::Regular {
                        *data
                    } else {
                        &b""[..]
                    },
                )
                .unwrap();
        }
        let tar_bytes = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar_bytes).unwrap();
        gz.finish().unwrap()
    }

    const R: tar::EntryType = tar::EntryType::Regular;

    #[test]
    fn extracts_the_exact_entry() {
        let a = tar_gz(&[
            ("xerj-1.2.3-t/README.md", b"readme", R),
            ("xerj-1.2.3-t/xerj", b"BINARY", R),
        ]);
        assert_eq!(extract_binary(&a, "1.2.3", "t").unwrap(), b"BINARY");
    }

    #[test]
    fn ignores_look_alike_entries() {
        let a = tar_gz(&[
            ("../xerj", b"evil", R),
            ("xerj", b"evil", R),
            ("xerj-1.2.3-t/xerj.exe", b"evil", R),
            ("other/xerj-1.2.3-t/xerj", b"evil", R),
        ]);
        assert!(extract_binary(&a, "1.2.3", "t").is_err());
    }

    #[test]
    fn a_symlink_entry_is_refused() {
        let a = tar_gz(&[("xerj-1.2.3-t/xerj", b"", tar::EntryType::Symlink)]);
        let e = extract_binary(&a, "1.2.3", "t").unwrap_err().to_string();
        assert!(e.contains("regular file"), "{e}");
    }

    #[test]
    fn not_a_tar_gz() {
        assert!(extract_binary(b"plain text", "1.2.3", "t").is_err());
    }

    #[test]
    fn an_empty_binary_is_refused() {
        let a = tar_gz(&[("xerj-1.2.3-t/xerj", b"", R)]);
        assert!(extract_binary(&a, "1.2.3", "t").is_err());
    }
}
