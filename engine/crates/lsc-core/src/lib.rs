//! Pure domain models and validation rules for `ls`.
//!
//! # Architecture & Invariants
//! Contains zero system call wrappers, zero filesystem operations, and zero network calls.
//! Provides pure LeanSpec AST parsing, formal structural validation rules (R01-R16),
//! Lean 4 theorem representations, and Content-Addressed Storage (CAS) listing types.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod cas;
pub mod document;
pub mod lean;
pub mod projectfile;
pub mod receipt;
pub mod rules;

pub use cas::{
    compute_blake3_hash, compute_cas_tree_hash, verify_cas_entry, CasEntry, CasEntryType,
    CasListing,
};
pub use document::{
    SpecDocument, SpecFrontmatter, SpecPriority, SpecSection, SpecStatus, ValidationMessage,
    ValidationReport,
};
pub use lean::{
    extract_lean_blocks, parse_lean_source, LeanConstructor, LeanDeclaration, LeanDef, LeanField,
    LeanInductive, LeanModule, LeanParameter, LeanStructure, LeanTheorem, ProofStatus,
    MAX_AST_RECURSION_DEPTH,
};
pub use projectfile::{
    compute_matrix_cells, find_references, find_spans, normalize_needs, CiMatrixDef, CiNeedOption,
    CiNode, CiResolveResult, CiResolver, GhaCompiler, GhaTargetOptions, ImageCellResult,
    ImageResolveResult, ImageResolver, JustTargetOptions, JustfileCompiler,
    ProjectfileInterpolator, ReferenceSpan, TemplateSpan, XtaskCompiler, XtaskGenerateError,
    XtaskTargetOptions,
};
pub use receipt::{
    parse_libtest_counts, LibtestCounts, ReceiptCacheState, ReceiptResult, ValidationGateReceipt,
    ValidationReceipt, ValidityFingerprint, ValidityInput, ValidityInputKind, ValidityStrength,
    VALIDATION_RECEIPT_SCHEMA_VERSION, VALIDITY_FINGERPRINT_SCHEMA_VERSION,
};
pub use rules::{
    parse_file_name, parse_folder_name, parse_folder_spec, parse_spec, LEGACY_SECTIONS,
    MODERN_SECTIONS,
};
