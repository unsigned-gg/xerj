// SPDX-License-Identifier: Apache-2.0 OR MIT
//! `lsc` — LeanSpec AST parser, architectural validator, and CAS listing engine CLI.

use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use lsc::{
    check_directory, format_report, parse_folder_name, parse_folder_spec, parse_spec,
    scan_cas_path, scan_specs_dir, SpecDocument, ValidationReport,
};

/// Parse one specification file. A `README.md` inside a `NNN-slug/` folder is a folder
/// specification whose stem is the folder name (SPEC-0005).
fn parse_spec_file(
    path: &Path,
    file_name: &str,
    content: &str,
    report: &mut ValidationReport,
) -> Option<SpecDocument> {
    if file_name == "README.md" {
        let folder = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str());
        if let Some(folder) = folder.filter(|f| parse_folder_name(f).is_some()) {
            return parse_folder_spec(folder, content, report);
        }
    }
    parse_spec(file_name, content, report)
}

fn print_usage() {
    println!("lsc — LeanSpec AST parser, architectural validator & CAS listing engine");
    println!();
    println!("USAGE:");
    println!("    lsc check [DIR]        Validate specifications in DIR (default: specs/)");
    println!(
        "    lsc hash [PATH]        Compute BLAKE3 CAS hash of file or directory (default: specs/)"
    );
    println!(
        "    lsc list [DIR]         List specifications with metadata in DIR (default: specs/)"
    );
    println!("    lsc parse <FILE>       Parse single specification file into JSON AST");
    println!("    lsc theorems [PATH]    List formal theorem statements & verification status");
    println!("    lsc pf interp <FILE>   Interpolate projectfile expressions (§3.8)");
    println!("    lsc pf ci resolve <F>  Resolve org.projectfile.ci intent DAG");
    println!(
        "    lsc pf ci generate <F> Lower DAG to Rust xtask, Justfile, or GHA (--target xtask|just|gha, --dispatch, --goal <node>)"
    );
    println!("    lsc pf image tags <F>  Derive org.projectfile.image variant tags");
    println!("    lsc --version, -v      Print version information");
    println!("    lsc --help, -h         Print this help message");
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        return ExitCode::SUCCESS;
    }

    match args[1].as_str() {
        "--version" | "-v" => {
            println!("lsc {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "--help" | "-h" => {
            print_usage();
            ExitCode::SUCCESS
        }
        "check" => {
            let target_dir = args.get(2).map(String::as_str).unwrap_or("specs");
            let path = Path::new(target_dir);

            match check_directory(path) {
                Ok((result, formatted)) => {
                    print!("{formatted}");
                    if result.report.is_clean() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    }
                }
                Err(err) => {
                    eprintln!("lsc: failed to scan directory '{}': {err}", path.display());
                    ExitCode::FAILURE
                }
            }
        }
        "hash" => {
            let target_path = args.get(2).map(String::as_str).unwrap_or("specs");
            let path = Path::new(target_path);

            match scan_cas_path(path) {
                Ok(digest) => {
                    println!("{digest}  {}", path.display());
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!(
                        "lsc: cannot compute CAS hash for '{}': {err}",
                        path.display()
                    );
                    ExitCode::FAILURE
                }
            }
        }
        "list" => {
            let target_dir = args.get(2).map(String::as_str).unwrap_or("specs");
            let path = Path::new(target_dir);

            match scan_specs_dir(path) {
                Ok(result) => {
                    println!(
                        "{:<6} {:<28} {:<12} {:<10} {:<8} {:<6} {:<16}",
                        "NUM", "SLUG", "STATUS", "PRIORITY", "THEOREMS", "LINES", "BLAKE3"
                    );
                    println!(
                        "{:-<6} {:-<28} {:-<12} {:-<10} {:-<8} {:-<6} {:-<16}",
                        "", "", "", "", "", "", ""
                    );
                    for doc in &result.documents {
                        let priority = doc
                            .frontmatter
                            .priority
                            .map(|p| format!("{p:?}"))
                            .unwrap_or_else(|| "-".to_owned());
                        let short_hash = if doc.content_hash.len() >= 16 {
                            &doc.content_hash[..16]
                        } else {
                            &doc.content_hash
                        };
                        println!(
                            "{:<6} {:<28} {:<12} {:<10} {:<8} {:<6} {:<16}",
                            doc.number,
                            doc.slug,
                            doc.frontmatter.status.as_str(),
                            priority,
                            doc.theorems.len(),
                            doc.line_count,
                            short_hash,
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("lsc: failed to scan directory '{}': {err}", path.display());
                    ExitCode::FAILURE
                }
            }
        }
        "parse" => {
            let Some(file_path) = args.get(2) else {
                eprintln!("lsc: parse requires a file path argument");
                return ExitCode::FAILURE;
            };
            let path = Path::new(file_path);
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(file_path);

            let content = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(err) => {
                    eprintln!("lsc: cannot read file '{}': {err}", path.display());
                    return ExitCode::FAILURE;
                }
            };

            let mut report = ValidationReport::default();
            match parse_spec_file(path, file_name, &content, &mut report) {
                Some(doc) => match serde_json::to_string_pretty(&doc) {
                    Ok(json) => {
                        println!("{json}");
                        ExitCode::SUCCESS
                    }
                    Err(err) => {
                        eprintln!("lsc: json serialization error: {err}");
                        ExitCode::FAILURE
                    }
                },
                None => {
                    eprintln!("lsc: failed to parse spec '{}'", file_name);
                    eprint!("{}", format_report(&report));
                    ExitCode::FAILURE
                }
            }
        }
        "theorems" => {
            let target_path = args.get(2).map(String::as_str).unwrap_or("specs");
            let path = Path::new(target_path);

            if path.is_file() {
                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(target_path);
                let content = match fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(err) => {
                        eprintln!("lsc: cannot read file '{}': {err}", path.display());
                        return ExitCode::FAILURE;
                    }
                };
                let mut report = ValidationReport::default();
                if let Some(doc) = parse_spec_file(path, file_name, &content, &mut report) {
                    print_doc_theorems(&doc);
                    ExitCode::SUCCESS
                } else {
                    eprintln!("lsc: failed to parse specification '{}'", file_name);
                    ExitCode::FAILURE
                }
            } else {
                match scan_specs_dir(path) {
                    Ok(result) => {
                        for doc in &result.documents {
                            print_doc_theorems(doc);
                        }
                        ExitCode::SUCCESS
                    }
                    Err(err) => {
                        eprintln!("lsc: failed to scan directory '{}': {err}", path.display());
                        ExitCode::FAILURE
                    }
                }
            }
        }
        "pf" => handle_projectfile_command(&args[2..]),
        unknown => {
            eprintln!("lsc: unrecognized command '{unknown}'");
            print_usage();
            ExitCode::FAILURE
        }
    }
}

fn handle_projectfile_command(args: &[String]) -> ExitCode {
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        println!("lsc pf — Projectfile v1 normative engine & DAG compiler");
        println!();
        println!("USAGE:");
        println!("    lsc pf interp <FILE> [--expr <EXPR>]    Interpolate projectfile (§3.8)");
        println!("    lsc pf ci resolve <FILE>                Resolve org.projectfile.ci DAG");
        println!("    lsc pf ci generate <FILE> [--target xtask|just|gha] [--dispatch] [--goal <node>] Lower DAG to Rust xtask, Justfile, or GitHub Actions");
        println!("    lsc pf image tags <FILE>                Derive org.projectfile.image tags");
        return ExitCode::SUCCESS;
    }

    match args[0].as_str() {
        "interp" => {
            let Some(file_path) = args.get(1) else {
                eprintln!("lsc pf: interp requires a file path argument");
                return ExitCode::FAILURE;
            };
            let content = match fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(err) => {
                    eprintln!("lsc pf: cannot read file '{file_path}': {err}");
                    return ExitCode::FAILURE;
                }
            };
            let val: serde_json::Value = match serde_yaml::from_str(&content) {
                Ok(v) => v,
                Err(err) => {
                    eprintln!("lsc pf: cannot parse YAML in '{file_path}': {err}");
                    return ExitCode::FAILURE;
                }
            };
            let interpolator = lsc::ProjectfileInterpolator::new(&val);
            if args.len() >= 4 && args[2] == "--expr" {
                let expr = &args[3];
                let out = interpolator.interpolate_string(expr, None, 0);
                println!("{out}");
            } else {
                let lines = interpolator.interpolate_lines(&content);
                for line in lines {
                    println!("{line}");
                }
            }
            ExitCode::SUCCESS
        }
        "ci" => {
            if args.len() < 2 {
                eprintln!("lsc pf ci: requires a subcommand: 'resolve' or 'generate'");
                return ExitCode::FAILURE;
            }
            match args[1].as_str() {
                "resolve" => {
                    let Some(file_path) = args.get(2) else {
                        eprintln!("lsc pf ci resolve: requires a file path argument");
                        return ExitCode::FAILURE;
                    };
                    let content = match fs::read_to_string(file_path) {
                        Ok(c) => c,
                        Err(err) => {
                            eprintln!("lsc pf ci resolve: cannot read file '{file_path}': {err}");
                            return ExitCode::FAILURE;
                        }
                    };
                    let val: serde_json::Value = match serde_yaml::from_str(&content) {
                        Ok(v) => v,
                        Err(err) => {
                            eprintln!(
                                "lsc pf ci resolve: cannot parse YAML in '{file_path}': {err}"
                            );
                            return ExitCode::FAILURE;
                        }
                    };
                    let ci_doc = val
                        .get("org")
                        .and_then(|o| o.get("projectfile"))
                        .and_then(|p| p.get("ci"))
                        .unwrap_or(&val);
                    let res = lsc::CiResolver::resolve_fixture(ci_doc);
                    let json = match serde_json::to_string_pretty(&res) {
                        Ok(j) => j,
                        Err(err) => {
                            eprintln!("lsc pf ci resolve: json serialization error: {err}");
                            return ExitCode::FAILURE;
                        }
                    };
                    println!("{json}");
                    if res.reject.is_some() {
                        ExitCode::FAILURE
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                "generate" => {
                    let Some(file_path) = args.get(2) else {
                        eprintln!("lsc pf ci generate: requires a file path argument");
                        return ExitCode::FAILURE;
                    };
                    let mut target = "xtask";
                    let mut dispatch = false;
                    let mut goal: Option<&str> = None;
                    let mut idx = 3;
                    while idx < args.len() {
                        if args[idx] == "--target" {
                            if let Some(val) = args.get(idx + 1) {
                                target = val.as_str();
                                idx += 2;
                                continue;
                            }
                            eprintln!(
                                "lsc pf ci generate: --target requires a value (xtask|just|gha)"
                            );
                            return ExitCode::FAILURE;
                        } else if let Some(val) = args[idx].strip_prefix("--target=") {
                            target = val;
                        } else if args[idx] == "--dispatch" {
                            dispatch = true;
                        } else if args[idx] == "--goal" {
                            if let Some(val) = args.get(idx + 1) {
                                goal = Some(val.as_str());
                                idx += 2;
                                continue;
                            }
                            eprintln!("lsc pf ci generate: --goal requires a node name");
                            return ExitCode::FAILURE;
                        } else if let Some(val) = args[idx].strip_prefix("--goal=") {
                            goal = Some(val);
                        }
                        idx += 1;
                    }
                    let content = match fs::read_to_string(file_path) {
                        Ok(c) => c,
                        Err(err) => {
                            eprintln!("lsc pf ci generate: cannot read file '{file_path}': {err}");
                            return ExitCode::FAILURE;
                        }
                    };
                    // SPEC-0003 requirement 18: the xtask target goes through the library
                    // entry point, so the CLI and a host xtask lower the same way.
                    if matches!(target, "xtask" | "rust") {
                        let options = lsc::XtaskTargetOptions {
                            dispatch,
                            ..lsc::XtaskTargetOptions::default()
                        };
                        return match lsc::XtaskCompiler::generate(&content, goal, &options) {
                            Ok(out) => {
                                print!("{out}");
                                ExitCode::SUCCESS
                            }
                            Err(lsc::XtaskGenerateError::Parse(err)) => {
                                eprintln!(
                                    "lsc pf ci generate: cannot parse YAML in '{file_path}': {err}"
                                );
                                ExitCode::FAILURE
                            }
                            Err(err) => {
                                eprintln!("lsc pf ci generate: {err}");
                                ExitCode::FAILURE
                            }
                        };
                    }
                    let val: serde_json::Value = match serde_yaml::from_str(&content) {
                        Ok(v) => v,
                        Err(err) => {
                            eprintln!(
                                "lsc pf ci generate: cannot parse YAML in '{file_path}': {err}"
                            );
                            return ExitCode::FAILURE;
                        }
                    };
                    let ci_doc = val
                        .get("org")
                        .and_then(|o| o.get("projectfile"))
                        .and_then(|p| p.get("ci"))
                        .unwrap_or(&val);
                    // SPEC-0003 requirements 11 and 12: one named goal, or the flagged goals.
                    let res = match goal {
                        Some(node) => lsc::CiResolver::resolve_goal(ci_doc, node),
                        None => lsc::CiResolver::resolve_fixture(ci_doc),
                    };
                    if let Some(ref reject) = res.reject {
                        eprintln!("lsc pf ci generate: DAG rejected with code: '{reject}'");
                        return ExitCode::FAILURE;
                    }
                    if dispatch && !matches!(target, "xtask" | "rust") {
                        eprintln!("lsc pf ci generate: --dispatch only applies to --target xtask");
                        return ExitCode::FAILURE;
                    }
                    if goal.is_some()
                        && !matches!(target, "xtask" | "rust" | "gha" | "github-actions")
                    {
                        eprintln!(
                            "lsc pf ci generate: --goal only applies to --target xtask or gha"
                        );
                        return ExitCode::FAILURE;
                    }
                    // SPEC-0003 requirement 14: a tool pin with a missing field is an error.
                    let incomplete = lsc::XtaskCompiler::incomplete_tools(&val);
                    if !incomplete.is_empty() {
                        eprintln!(
                            "lsc pf ci generate: DAG rejected with code: 'incomplete-tool' (gg.unsigned.ls.tools: {}; each row needs string crate, version and bin)",
                            incomplete.join(", ")
                        );
                        return ExitCode::FAILURE;
                    }
                    match target {
                        "just" | "justfile" => {
                            let options = lsc::JustTargetOptions::default();
                            let out = lsc::JustfileCompiler::compile_from_doc(&val, &res, &options);
                            print!("{out}");
                            ExitCode::SUCCESS
                        }
                        "gha" | "github-actions" => {
                            let mut options = lsc::GhaTargetOptions::default();
                            if let Some(node) = goal {
                                options.default_target = node.to_string();
                            }
                            let out = lsc::GhaCompiler::compile_from_doc(&val, &res, &options);
                            print!("{out}");
                            ExitCode::SUCCESS
                        }
                        other => {
                            eprintln!(
                                "lsc pf ci generate: unsupported --target '{other}' (expected 'xtask', 'just', or 'gha')"
                            );
                            ExitCode::FAILURE
                        }
                    }
                }
                unknown => {
                    eprintln!("lsc pf ci: unrecognized subcommand '{unknown}'");
                    ExitCode::FAILURE
                }
            }
        }
        "image" => {
            if args.len() < 2 {
                eprintln!("lsc pf image: requires a subcommand: 'tags'");
                return ExitCode::FAILURE;
            }
            match args[1].as_str() {
                "tags" => {
                    let Some(file_path) = args.get(2) else {
                        eprintln!("lsc pf image tags: requires a file path argument");
                        return ExitCode::FAILURE;
                    };
                    let content = match fs::read_to_string(file_path) {
                        Ok(c) => c,
                        Err(err) => {
                            eprintln!("lsc pf image tags: cannot read file '{file_path}': {err}");
                            return ExitCode::FAILURE;
                        }
                    };
                    let val: serde_json::Value = match serde_yaml::from_str(&content) {
                        Ok(v) => v,
                        Err(err) => {
                            eprintln!(
                                "lsc pf image tags: cannot parse YAML in '{file_path}': {err}"
                            );
                            return ExitCode::FAILURE;
                        }
                    };
                    let res = lsc::ImageResolver::resolve_fixture(&val);
                    let json = match serde_json::to_string_pretty(&res) {
                        Ok(j) => j,
                        Err(err) => {
                            eprintln!("lsc pf image tags: json serialization error: {err}");
                            return ExitCode::FAILURE;
                        }
                    };
                    println!("{json}");
                    ExitCode::SUCCESS
                }
                unknown => {
                    eprintln!("lsc pf image: unrecognized subcommand '{unknown}'");
                    ExitCode::FAILURE
                }
            }
        }
        unknown => {
            eprintln!("lsc pf: unrecognized command '{unknown}'");
            ExitCode::FAILURE
        }
    }
}

fn print_doc_theorems(doc: &lsc::SpecDocument) {
    if doc.theorems.is_empty() {
        return;
    }
    println!(
        "Specification: {} ({})",
        doc.stem,
        doc.frontmatter.status.as_str()
    );
    println!(
        "{:<28} {:<12} {:<10} {:<30}",
        "THEOREM", "STATUS", "TACTICS", "SIGNATURE"
    );
    println!("{:-<28} {:-<12} {:-<10} {:-<30}", "", "", "", "");
    for th in &doc.theorems {
        let tactics_str = if th.tactics.is_empty() {
            "-".to_owned()
        } else {
            th.tactics.join(",")
        };
        let status_str = match th.proof_status {
            lsc::ProofStatus::Proved { tactic_count } => format!("proved({tactic_count})"),
            lsc::ProofStatus::Sorry => "sorry".to_owned(),
            lsc::ProofStatus::Axiom => "axiom".to_owned(),
            lsc::ProofStatus::Omitted => "omitted".to_owned(),
        };
        println!(
            "{:<28} {:<12} {:<10} {:<30}",
            th.name, status_str, tactics_str, th.signature
        );
    }
    println!();
}
