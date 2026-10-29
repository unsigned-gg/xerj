//! Content-Addressed Storage (CAS) entry and tree listing representations.
//!
//! Pure data structures and BLAKE3 hashing algorithms for directory trees and
//! content-addressed storage manifests. Contains zero system I/O.

use serde::{Deserialize, Serialize};

/// Type classification of an entry in a content-addressed tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CasEntryType {
    /// Standard file / binary blob.
    Blob,
    /// Executable binary or script.
    Executable,
    /// Directory tree.
    Tree,
    /// Symbolic link target.
    Symlink,
}

/// A content-addressed entry within a storage manifest or tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CasEntry {
    /// Cryptographic digest (BLAKE3 hex string).
    pub hash: String,
    /// Relative path within the scanned root.
    pub rel_path: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Entry type classification.
    pub entry_type: CasEntryType,
}

impl CasEntry {
    /// Create a new CAS entry.
    #[must_use]
    pub fn new(
        hash: impl Into<String>,
        rel_path: impl Into<String>,
        size: u64,
        kind: CasEntryType,
    ) -> Self {
        Self {
            hash: hash.into(),
            rel_path: rel_path.into(),
            size_bytes: size,
            entry_type: kind,
        }
    }
}

/// Computes the 256-bit BLAKE3 hash of an in-memory byte slice as a hex string.
#[must_use]
pub fn compute_blake3_hash(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

/// Verifies whether the provided byte slice matches the expected BLAKE3 hash.
#[must_use]
pub fn verify_cas_entry(data: &[u8], expected_hash: &str) -> bool {
    compute_blake3_hash(data).eq_ignore_ascii_case(expected_hash)
}

/// Computes the deterministic Merkle root hash for an ordered slice of CAS entries.
#[must_use]
pub fn compute_cas_tree_hash(entries: &[CasEntry]) -> String {
    let mut hasher = blake3::Hasher::new();
    for entry in entries {
        hasher.update(entry.rel_path.as_bytes());
        hasher.update(b":");
        hasher.update(entry.hash.as_bytes());
        hasher.update(b":");
        hasher.update(&entry.size_bytes.to_le_bytes());
        hasher.update(b":");
        hasher.update(match entry.entry_type {
            CasEntryType::Blob => b"blob",
            CasEntryType::Executable => b"exe",
            CasEntryType::Tree => b"tree",
            CasEntryType::Symlink => b"symlink",
        });
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex().to_string()
}

/// Aggregated listing of a content-addressed tree.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CasListing {
    /// Root hash representing the collective tree state.
    pub root_hash: String,
    /// All indexed entries ordered by path.
    pub entries: Vec<CasEntry>,
    /// Total aggregated size in bytes.
    pub total_bytes: u64,
}

impl CasListing {
    /// Construct a new `CasListing` from entries, sorting paths and calculating root hash.
    #[must_use]
    pub fn from_entries(mut entries: Vec<CasEntry>) -> Self {
        entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        let total_bytes = entries.iter().map(|e| e.size_bytes).sum();
        let root_hash = compute_cas_tree_hash(&entries);
        Self {
            root_hash,
            entries,
            total_bytes,
        }
    }

    /// Verifies the structural integrity of the listing.
    #[must_use]
    pub fn verify_root(&self) -> bool {
        compute_cas_tree_hash(&self.entries) == self.root_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blake3_hash_deterministic() {
        let data = b"LeanSpec AST CAS Hash Verification";
        let h1 = compute_blake3_hash(data);
        let h2 = compute_blake3_hash(data);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert!(verify_cas_entry(data, &h1));
    }

    #[test]
    fn test_cas_listing_construction() {
        let e1 = CasEntry::new(
            compute_blake3_hash(b"file1"),
            "specs/0001.md",
            5,
            CasEntryType::Blob,
        );
        let e2 = CasEntry::new(
            compute_blake3_hash(b"file2"),
            "specs/0002.md",
            5,
            CasEntryType::Blob,
        );
        let listing = CasListing::from_entries(vec![e2, e1]);
        assert_eq!(listing.entries[0].rel_path, "specs/0001.md");
        assert_eq!(listing.entries[1].rel_path, "specs/0002.md");
        assert_eq!(listing.total_bytes, 10);
        assert!(listing.verify_root());
    }
}
