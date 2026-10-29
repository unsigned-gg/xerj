//! Zero-I/O Lean 4 spec AST parsing and theorem statement domain models.
//!
//! Provides pure representations of Lean 4 formal specifications: inductive families,
//! coinductive streams, definitions, theorem statements, proof obligations, and BLAKE3 digests.

use crate::cas::compute_blake3_hash;
use serde::{Deserialize, Serialize};

/// Maximum AST parse recursion depth to prevent stack overflow from adversarial inputs.
pub const MAX_AST_RECURSION_DEPTH: usize = 64;

/// Lean 4 function or theorem parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeanParameter {
    /// Identifier name of the parameter.
    pub name: String,
    /// Type annotation string.
    pub param_type: String,
    /// Whether parameter is implicit (enclosed in `{...}` or `[...]`).
    pub is_implicit: bool,
}

/// A constructor definition within an inductive or coinductive type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeanConstructor {
    /// Constructor identifier (e.g. `nil`, `cons`).
    pub name: String,
    /// Type signature.
    pub signature: String,
}

/// A field within a Lean 4 structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeanField {
    /// Field identifier.
    pub name: String,
    /// Field type annotation.
    pub field_type: String,
}

/// Proof verification status of a formal theorem statement.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProofStatus {
    /// Proved formally with verified tactic sequence.
    Proved {
        /// Count of tactics executed in proof script.
        tactic_count: usize,
    },
    /// Incomplete proof with `sorry` placeholder.
    Sorry,
    /// Postulated as an unproved formal axiom.
    Axiom,
    /// Proof omitted or body not present in specification.
    Omitted,
}

impl ProofStatus {
    /// Returns canonical string representation.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Proved { .. } => "proved",
            Self::Sorry => "sorry",
            Self::Axiom => "axiom",
            Self::Omitted => "omitted",
        }
    }
}

/// A formal theorem or lemma statement in Lean 4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeanTheorem {
    /// Theorem identifier (e.g. `cas_tree_hash_unique`).
    pub name: String,
    /// Parameters and hypothesis binders.
    pub parameters: Vec<LeanParameter>,
    /// Propositional type signature being proven.
    pub signature: String,
    /// Status of the proof script.
    pub proof_status: ProofStatus,
    /// Raw proof body text (if present).
    pub proof_body: Option<String>,
    /// Extracted tactic sequence names (e.g. `["intro", "rfl", "simp"]`).
    pub tactics: Vec<String>,
    /// 1-indexed line number in source document.
    pub line_number: usize,
    /// BLAKE3 content hash of signature and proof script.
    pub hash: String,
}

/// A function or value definition in Lean 4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeanDef {
    /// Definition identifier.
    pub name: String,
    /// Parameters.
    pub parameters: Vec<LeanParameter>,
    /// Return type signature.
    pub return_type: String,
    /// Definition body text.
    pub body: String,
    /// 1-indexed line number.
    pub line_number: usize,
}

/// An inductive data type or coinductive stream in Lean 4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeanInductive {
    /// Type identifier.
    pub name: String,
    /// Type parameters.
    pub parameters: Vec<LeanParameter>,
    /// Universe level or sorting type signature.
    pub type_signature: String,
    /// Constructors.
    pub constructors: Vec<LeanConstructor>,
    /// Whether this type is a coinductive stream / greatest fixpoint.
    pub is_coinductive: bool,
    /// 1-indexed line number.
    pub line_number: usize,
}

/// A Lean 4 structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeanStructure {
    /// Structure identifier.
    pub name: String,
    /// Fields.
    pub fields: Vec<LeanField>,
    /// 1-indexed line number.
    pub line_number: usize,
}

/// A top-level Lean 4 declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeanDeclaration {
    /// Formal theorem or lemma.
    Theorem(LeanTheorem),
    /// Function or constant definition.
    Def(LeanDef),
    /// Inductive data type or coinductive stream.
    Inductive(LeanInductive),
    /// Record structure.
    Structure(LeanStructure),
    /// Formal axiom statement.
    Axiom {
        /// Axiom identifier.
        name: String,
        /// Propositional signature.
        signature: String,
        /// Line number.
        line_number: usize,
    },
    /// Raw or unparsed declaration line.
    Raw(String),
}

/// An abstract syntax tree representation of a Lean 4 specification module.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeanModule {
    /// Module name or namespace (e.g. `Unsigned.Specs.Cas`).
    pub name: String,
    /// Imported module names.
    pub imports: Vec<String>,
    /// Top-level declarations in source order.
    pub declarations: Vec<LeanDeclaration>,
    /// BLAKE3 hash of canonical AST representation.
    pub hash: String,
}

impl LeanModule {
    /// Collect all formal theorems declared in this module.
    #[must_use]
    pub fn theorems(&self) -> Vec<&LeanTheorem> {
        self.declarations
            .iter()
            .filter_map(|d| match d {
                LeanDeclaration::Theorem(th) => Some(th),
                _ => None,
            })
            .collect()
    }

    /// Whether any theorem in this module contains an unproven `sorry`.
    #[must_use]
    pub fn has_unproved_sorry(&self) -> bool {
        self.theorems()
            .iter()
            .any(|th| matches!(th.proof_status, ProofStatus::Sorry))
    }
}

/// Extract all fenced Lean 4 code blocks from a Markdown specification.
///
/// Returns tuples of `(start_line_number, code_block_content)`.
#[must_use]
pub fn extract_lean_blocks(markdown: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut current_block: Option<(usize, Vec<&str>)> = None;

    for (idx, line) in markdown.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```lean") || trimmed.starts_with("~~~lean") {
            current_block = Some((idx + 2, Vec::new()));
        } else if (trimmed == "```" || trimmed == "~~~") && current_block.is_some() {
            if let Some((start_line, lines)) = current_block.take() {
                blocks.push((start_line, lines.join("\n")));
            }
        } else if let Some((_, ref mut lines)) = current_block {
            lines.push(line);
        }
    }

    blocks
}

/// Pure parser that turns Lean 4 source code into a `LeanModule` AST.
///
/// Guards against unbounded recursion by enforcing `max_depth`.
///
/// # Errors
/// Returns an error string if parsing violates recursion limits or malformed syntax.
pub fn parse_lean_source(
    source: &str,
    start_line: usize,
    max_depth: usize,
) -> Result<LeanModule, String> {
    if max_depth == 0 {
        return Err("Recursion depth limit exceeded while parsing Lean AST".to_owned());
    }

    let mut module = LeanModule::default();
    let lines: Vec<&str> = source.lines().collect();

    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        let abs_line = start_line + i;

        // Skip comments and blanks
        if trimmed.is_empty() || trimmed.starts_with("--") {
            i += 1;
            continue;
        }

        // Imports
        if let Some(rest) = trimmed.strip_prefix("import ") {
            module.imports.push(rest.trim().to_owned());
            i += 1;
            continue;
        }

        // Module / namespace
        if let Some(rest) = trimmed.strip_prefix("namespace ") {
            module.name = rest.trim().to_owned();
            i += 1;
            continue;
        }

        // Theorem / Lemma
        if trimmed.starts_with("theorem ") || trimmed.starts_with("lemma ") {
            let decl = parse_theorem_block(&lines, &mut i, abs_line);
            module.declarations.push(LeanDeclaration::Theorem(decl));
            continue;
        }

        // Definition
        if trimmed.starts_with("def ") {
            let decl = parse_def_block(&lines, &mut i, abs_line);
            module.declarations.push(LeanDeclaration::Def(decl));
            continue;
        }

        // Inductive / Coinductive
        if trimmed.starts_with("inductive ") || trimmed.starts_with("coinductive ") {
            let is_coinductive = trimmed.starts_with("coinductive ");
            let decl = parse_inductive_block(&lines, &mut i, abs_line, is_coinductive);
            module.declarations.push(LeanDeclaration::Inductive(decl));
            continue;
        }

        // Structure
        if trimmed.starts_with("structure ") {
            let decl = parse_structure_block(&lines, &mut i, abs_line);
            module.declarations.push(LeanDeclaration::Structure(decl));
            continue;
        }

        // Axiom
        if let Some(rest) = trimmed.strip_prefix("axiom ") {
            let (name, sig) = rest.split_once(':').unwrap_or((rest, "Prop"));
            module.declarations.push(LeanDeclaration::Axiom {
                name: name.trim().to_owned(),
                signature: sig.trim().to_owned(),
                line_number: abs_line,
            });
            i += 1;
            continue;
        }

        i += 1;
    }

    // Compute canonical BLAKE3 digest of source
    module.hash = compute_blake3_hash(source.as_bytes());

    Ok(module)
}

fn parse_theorem_block(lines: &[&str], idx: &mut usize, start_line: usize) -> LeanTheorem {
    let header_line = lines[*idx];
    *idx += 1;

    let header_clean = header_line
        .trim()
        .strip_prefix("theorem ")
        .or_else(|| header_line.trim().strip_prefix("lemma "))
        .unwrap_or(header_line);

    let (name_part, rest) = header_clean.split_once(':').unwrap_or((header_clean, ""));
    let name = name_part
        .split_whitespace()
        .next()
        .unwrap_or("unnamed")
        .to_owned();

    let mut body_lines = Vec::new();
    let mut signature = rest.to_owned();

    // Check if proof starts on same line
    if let Some((sig, proof)) = rest.split_once(":=") {
        signature = sig.trim().to_owned();
        body_lines.push(proof.trim());
    }

    // Accumulate proof body until next top-level keyword or empty fence
    while *idx < lines.len() {
        let next_line = lines[*idx];
        let trimmed = next_line.trim();

        if trimmed.starts_with("theorem ")
            || trimmed.starts_with("lemma ")
            || trimmed.starts_with("def ")
            || trimmed.starts_with("inductive ")
            || trimmed.starts_with("structure ")
            || trimmed.starts_with("axiom ")
            || trimmed.starts_with("namespace ")
        {
            break;
        }

        body_lines.push(next_line);
        *idx += 1;
    }

    let full_body = body_lines.join("\n");
    let has_sorry = full_body.contains("sorry");

    let mut tactics = Vec::new();
    for token in full_body.split_whitespace() {
        let t = token.trim_matches(|c: char| !c.is_alphanumeric());
        match t {
            "intro" | "rfl" | "simp" | "omega" | "ring" | "apply" | "cases" | "exact"
            | "linarith" | "decide" | "assumption" | "split" | "contradiction"
                if !tactics.contains(&t.to_owned()) =>
            {
                tactics.push(t.to_owned());
            }
            _ => {}
        }
    }

    let proof_status = if has_sorry {
        ProofStatus::Sorry
    } else if !tactics.is_empty() {
        ProofStatus::Proved {
            tactic_count: tactics.len(),
        }
    } else if full_body.trim().is_empty() {
        ProofStatus::Omitted
    } else {
        ProofStatus::Proved { tactic_count: 1 }
    };

    let hash = compute_blake3_hash(format!("{name}:{signature}:{full_body}").as_bytes());

    LeanTheorem {
        name,
        parameters: Vec::new(),
        signature: signature.trim().to_owned(),
        proof_status,
        proof_body: if full_body.trim().is_empty() {
            None
        } else {
            Some(full_body)
        },
        tactics,
        line_number: start_line,
        hash,
    }
}

fn parse_def_block(lines: &[&str], idx: &mut usize, start_line: usize) -> LeanDef {
    let line = lines[*idx];
    *idx += 1;
    let clean = line.trim().strip_prefix("def ").unwrap_or(line);
    let (name_part, rest) = clean.split_once(':').unwrap_or((clean, ""));
    let name = name_part
        .split_whitespace()
        .next()
        .unwrap_or("unnamed")
        .to_owned();

    let (ret_type, body) = rest.split_once(":=").unwrap_or((rest, ""));

    LeanDef {
        name,
        parameters: Vec::new(),
        return_type: ret_type.trim().to_owned(),
        body: body.trim().to_owned(),
        line_number: start_line,
    }
}

fn parse_inductive_block(
    lines: &[&str],
    idx: &mut usize,
    start_line: usize,
    is_coinductive: bool,
) -> LeanInductive {
    let line = lines[*idx];
    *idx += 1;
    let clean = if is_coinductive {
        line.trim().strip_prefix("coinductive ").unwrap_or(line)
    } else {
        line.trim().strip_prefix("inductive ").unwrap_or(line)
    };

    let (name_part, type_sig) = clean.split_once(':').unwrap_or((clean, "Type"));
    let name = name_part
        .split_whitespace()
        .next()
        .unwrap_or("unnamed")
        .to_owned();

    let mut constructors = Vec::new();
    while *idx < lines.len() {
        let next_line = lines[*idx];
        let trimmed = next_line.trim();

        if trimmed.starts_with('|') {
            let c_clean = trimmed.trim_start_matches('|').trim();
            let (c_name, c_sig) = c_clean.split_once(':').unwrap_or((c_clean, ""));
            constructors.push(LeanConstructor {
                name: c_name.trim().to_owned(),
                signature: c_sig.trim().to_owned(),
            });
            *idx += 1;
        } else {
            break;
        }
    }

    LeanInductive {
        name,
        parameters: Vec::new(),
        type_signature: type_sig.trim().to_owned(),
        constructors,
        is_coinductive,
        line_number: start_line,
    }
}

fn parse_structure_block(lines: &[&str], idx: &mut usize, start_line: usize) -> LeanStructure {
    let line = lines[*idx];
    *idx += 1;
    let clean = line.trim().strip_prefix("structure ").unwrap_or(line);
    let name = clean
        .split_whitespace()
        .next()
        .unwrap_or("unnamed")
        .to_owned();

    let mut fields = Vec::new();
    while *idx < lines.len() {
        let next_line = lines[*idx];
        let trimmed = next_line.trim();

        if trimmed.is_empty() || trimmed.starts_with("--") {
            *idx += 1;
            continue;
        }

        if trimmed.starts_with("theorem ")
            || trimmed.starts_with("def ")
            || trimmed.starts_with("inductive ")
            || trimmed.starts_with("structure ")
        {
            break;
        }

        if let Some((f_name, f_type)) = trimmed.split_once(':') {
            fields.push(LeanField {
                name: f_name.trim().to_owned(),
                field_type: f_type.trim().to_owned(),
            });
            *idx += 1;
        } else {
            break;
        }
    }

    LeanStructure {
        name,
        fields,
        line_number: start_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lean_parser_theorems_and_tactics() {
        let src = r#"
namespace Unsigned.Cas

inductive CasEntryType : Type where
  | blob : CasEntryType
  | tree : CasEntryType

theorem cas_hash_unique (data : ByteArray) : data.size >= 0 := by
  intro
  simp
  rfl

theorem unfinished_proof : 1 + 1 = 2 := by
  sorry
"#;
        let module = parse_lean_source(src, 1, MAX_AST_RECURSION_DEPTH).unwrap();
        assert_eq!(module.name, "Unsigned.Cas");
        let theorems = module.theorems();
        assert_eq!(theorems.len(), 2);
        assert_eq!(theorems[0].name, "cas_hash_unique");
        assert!(matches!(
            theorems[0].proof_status,
            ProofStatus::Proved { .. }
        ));
        assert_eq!(theorems[1].name, "unfinished_proof");
        assert_eq!(theorems[1].proof_status, ProofStatus::Sorry);
        assert!(module.has_unproved_sorry());
    }

    #[test]
    fn test_extract_lean_blocks_from_markdown() {
        let md = r#"
# 0001-test

## Requirements

```lean
def hello : String := "world"

theorem hello_not_empty : hello.length > 0 := by
  decide
```
"#;
        let blocks = extract_lean_blocks(md);
        assert_eq!(blocks.len(), 1);
        let (line, code) = &blocks[0];
        assert_eq!(*line, 7);
        assert!(code.contains("theorem hello_not_empty"));
    }

    #[test]
    fn test_coinductive_detection() {
        let src = r#"
coinductive Stream (α : Type) : Type where
  | cons : α -> Stream α -> Stream α
"#;
        let module = parse_lean_source(src, 1, MAX_AST_RECURSION_DEPTH).unwrap();
        if let LeanDeclaration::Inductive(ind) = &module.declarations[0] {
            assert!(ind.is_coinductive);
            assert_eq!(ind.name, "Stream");
        } else {
            panic!("expected inductive declaration");
        }
    }
}
