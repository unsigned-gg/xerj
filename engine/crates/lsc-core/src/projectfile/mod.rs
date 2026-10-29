// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Projectfile v1 normative engine, CI DAG resolution, and container image derivation.
//!
//! Conforms to normative specification <https://projectfile.org/spec/v1>,
//! `org.projectfile.ci`, and `org.projectfile.image`.
//!
//! # Invariants
//! - Zero-I/O: operates strictly over in-memory ASTs, JSON values, and formatted outputs.
//! - Deterministic execution orders and tag matrices.

pub mod ci;
pub mod gha_target;
pub mod image;
pub mod interp;
pub mod just_target;
pub mod xtask_target;

pub use ci::{
    compute_matrix_cells, normalize_needs, CiMatrixDef, CiNeedOption, CiNode, CiResolveResult,
    CiResolver,
};
pub use gha_target::{GhaCompiler, GhaTargetOptions};
pub use image::{ImageCellResult, ImageResolveResult, ImageResolver};
pub use interp::{
    find_references, find_spans, ProjectfileInterpolator, ReferenceSpan, TemplateSpan,
};
pub use just_target::{JustTargetOptions, JustfileCompiler};
pub use xtask_target::{XtaskCompiler, XtaskGenerateError, XtaskTargetOptions};
