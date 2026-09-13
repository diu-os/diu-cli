// diu — the DIU OS userland SDK/CLI (ADR D-123 stage 2).
//
// Three verbs, mirroring the kernel syscall surface for gene authors:
//   diu new <module_id>          scaffold a gene package (gene.json + README)
//   diu test [path] [--tier N]   run the kernel's Gate 1 validator LOCALLY —
//                                the exact same code the server runs on publish
//                                (diu_kernel::gate1), zero drift by construction
//   diu publish [path] --project <uuid>
//                                create a draft on a platform instance and
//                                publish it (server re-runs every gate; the
//                                server is always authoritative)
//
// Env for publish: DIU_API (default https://diu-os.com), DIU_TOKEN (JWT, required).
//
// What the local gate CANNOT check (server-only, by design):
//   - Q-96 retraction: the retracted_dois list lives on the instance;
//   - Gate 2 (Quantum review), engine/last_run presence, project ownership.
// `diu test` says so explicitly instead of pretending (K-4: no false "verified").

use diu_kernel::gate1::{validate_gate1, ValidateGate1Options};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const DEFAULT_API: &str = "https://diu-os.com";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("new") => cmd_new(&args[1..]),
        Some("test") => cmd_test(&args[1..]),
        Some("publish") => cmd_publish(&args[1..]),
        Some("--help") | Some("-h") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("diu: unknown command `{other}`\n");
            print_help();
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "diu — DIU OS gene SDK (kernel {})\n\n\
         USAGE:\n\
         \x20 diu new <module_id>                 scaffold a gene package\n\
         \x20 diu test [gene.json] [--tier N]     run kernel Gate 1 locally (default tier 2)\n\
         \x20 diu publish [gene.json] --project <uuid>\n\
         \x20                                     create draft + publish on a platform instance\n\n\
         ENV (publish):\n\
         \x20 DIU_API     instance base URL (default {DEFAULT_API})\n\
         \x20 DIU_TOKEN   JWT bearer token (required)\n\n\
         The local gate is the byte-identical validator the server runs (diu-kernel).\n\
         Server-only checks (DOI retraction Q-96, Gate 2 review, ownership) still\n\
         apply on publish — the platform instance is always authoritative.",
        env!("CARGO_PKG_VERSION"),
    );
}

// ── diu new ───────────────────────────────────────────────────────────────────

fn valid_module_id(s: &str) -> bool {
    let b = s.as_bytes();
    (3..=64).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

fn cmd_new(args: &[String]) -> ExitCode {
    let Some(module_id) = args.first() else {
        eprintln!("diu new: missing <module_id> (^[a-z][a-z0-9_]{{2,63}}$)");
        return ExitCode::FAILURE;
    };
    if !valid_module_id(module_id) {
        eprintln!("diu new: module_id must match ^[a-z][a-z0-9_]{{2,63}}$ (got `{module_id}`)");
        return ExitCode::FAILURE;
    }
    let dir = PathBuf::from(module_id);
    if dir.exists() {
        eprintln!("diu new: `{module_id}/` already exists");
        return ExitCode::FAILURE;
    }

    let doc = json!({
        "identity": {
            "module_id": module_id,
            "version": "0.1.0",
            "title": "REPLACE: human-readable module title (≤ 120 chars)",
            "author_orcid": ""
        },
        "scientific_purpose": "REPLACE: what scientific question this gene answers and why the model form is appropriate (min 40 chars — protected block).",
        "mathematical_model": {
            "equations": ["REPLACE: governing equation(s) as KaTeX, e.g. \\frac{dN}{dt} = rN(1 - N/K)"],
            "source_doi": ""
        },
        "inputs": [
            { "key": "example_param", "data_type": "scalar", "unit": "REPLACE: physical unit (Q-97)" }
        ],
        "outputs": [
            { "key": "example_series", "data_type": "series", "unit": "REPLACE: physical unit (Q-97)" }
        ],
        "epistemic": {
            "default_type": "hypothesis",
            "max_confidence": "hypothesis"
        },
        "known_limitations": "REPLACE: domain of validity and what the model deliberately ignores (min 40 chars — protected block).",
        "verification_test": "REPLACE: one reproducible check (min 20 chars)."
    });

    let readme = format!(
        "# {module_id} — DIU OS gene\n\n\
         Fill `gene.json`, then:\n\n\
         ```bash\n\
         diu test {module_id}/gene.json   # kernel Gate 1, locally\n\
         DIU_TOKEN=... diu publish {module_id}/gene.json --project <uuid>\n\
         ```\n\n\
         Gate 1 requires (D-070 §2.5): module_id/semver/title; scientific_purpose ≥ 40 chars;\n\
         ≥ 1 equation **with `source_doi`** (a real DOI — never fabricated, Q-95); every numeric\n\
         port with a physical unit (Q-97); epistemic `default_type` ≤ `max_confidence` ≤ the\n\
         platform ceiling (`hypothesis`); known_limitations ≥ 40 chars; a verification_test.\n\n\
         Server-side on publish: DOI retraction list (Q-96), Gate 2 (Quantum review), ownership.\n"
    );

    if let Err(e) = std::fs::create_dir_all(&dir)
        .and_then(|_| {
            std::fs::write(
                dir.join("gene.json"),
                serde_json::to_string_pretty(&doc).expect("serialize skeleton") + "\n",
            )
        })
        .and_then(|_| std::fs::write(dir.join("README.md"), readme))
    {
        eprintln!("diu new: {e}");
        return ExitCode::FAILURE;
    }
    println!("created {module_id}/gene.json and {module_id}/README.md");
    println!("next: edit gene.json, then `diu test {module_id}/gene.json`");
    ExitCode::SUCCESS
}

// ── diu test ──────────────────────────────────────────────────────────────────

fn load_doc(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: invalid JSON: {e}", path.display()))
}

fn parse_path_and_flag(args: &[String], flag: &str) -> (PathBuf, Option<String>) {
    let mut path = PathBuf::from("gene.json");
    let mut value = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag && i + 1 < args.len() {
            value = Some(args[i + 1].clone());
            i += 2;
        } else {
            path = PathBuf::from(&args[i]);
            i += 1;
        }
    }
    (path, value)
}

fn cmd_test(args: &[String]) -> ExitCode {
    let (path, tier) = parse_path_and_flag(args, "--tier");
    let tier: i32 = tier.as_deref().unwrap_or("2").parse().unwrap_or(2);

    let doc = match load_doc(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("diu test: {e}");
            return ExitCode::FAILURE;
        }
    };

    let failures = validate_gate1(
        &doc,
        ValidateGate1Options {
            subscription_tier: tier,
            ..Default::default()
        },
    );

    if failures.is_empty() {
        println!("Gate 1: PASS ({}, tier {tier})", path.display());
        println!(
            "note: server-only checks still apply on publish — DOI retraction (Q-96),\n\
             Gate 2 review, ownership. Local PASS is necessary, not sufficient."
        );
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "Gate 1: FAIL ({}, tier {tier}) — {} issue(s):",
            path.display(),
            failures.len()
        );
        for f in &failures {
            eprintln!("  [{}] {}", f.block_id, f.message);
        }
        ExitCode::FAILURE
    }
}

// ── diu publish ───────────────────────────────────────────────────────────────

fn cmd_publish(args: &[String]) -> ExitCode {
    let (path, project) = parse_path_and_flag(args, "--project");
    let Some(project_id) = project else {
        eprintln!("diu publish: --project <uuid> is required");
        return ExitCode::FAILURE;
    };
    let Ok(token) = std::env::var("DIU_TOKEN") else {
        eprintln!("diu publish: DIU_TOKEN env var (JWT) is required");
        return ExitCode::FAILURE;
    };
    let api = std::env::var("DIU_API").unwrap_or_else(|_| DEFAULT_API.to_string());

    let doc = match load_doc(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("diu publish: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Run the local kernel gate first: refuse to spend a server round-trip on a
    // doc that the byte-identical validator already rejects.
    let failures = validate_gate1(&doc, ValidateGate1Options::default());
    if !failures.is_empty() {
        eprintln!("diu publish: local Gate 1 FAIL — fix these first (`diu test`):");
        for f in &failures {
            eprintln!("  [{}] {}", f.block_id, f.message);
        }
        return ExitCode::FAILURE;
    }

    let client = reqwest::blocking::Client::new();

    // 1. create draft
    let create_url = format!("{api}/api/v1/projects/{project_id}/drafts");
    let resp = match client
        .post(&create_url)
        .bearer_auth(&token)
        .json(&json!({ "doc": doc, "phase": "draft" }))
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("diu publish: POST {create_url}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !resp.status().is_success() {
        eprintln!(
            "diu publish: create draft failed: {} {}",
            resp.status(),
            resp.text().unwrap_or_default()
        );
        return ExitCode::FAILURE;
    }
    let draft: Value = match resp.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("diu publish: create draft: invalid response: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(draft_id) = draft.get("draft_id").and_then(|v| v.as_str()) else {
        eprintln!("diu publish: create draft: no draft_id in response");
        return ExitCode::FAILURE;
    };
    println!("draft created: {draft_id}");

    // 2. publish (server re-runs Gate 1 + retraction + everything else)
    let publish_url = format!("{api}/api/v1/projects/{project_id}/drafts/{draft_id}/publish");
    let resp = match client.post(&publish_url).bearer_auth(&token).send() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("diu publish: POST {publish_url}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    if status.is_success() {
        println!("published: {body}");
        ExitCode::SUCCESS
    } else {
        eprintln!("diu publish: server refused ({status}): {body}");
        eprintln!("(the instance is authoritative — e.g. Q-96 retraction or Gate 2 may apply)");
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_skeleton_fails_gate1_until_filled() {
        // The skeleton must NOT pass Gate 1 as-is: placeholders are not science.
        // (empty source_doi with a declared equation → Q-95.)
        let doc = json!({
            "identity": { "module_id": "abc_gene", "version": "0.1.0", "title": "T", "author_orcid": "" },
            "scientific_purpose": "x",
            "mathematical_model": { "equations": ["x=1"], "source_doi": "" },
            "inputs": [], "outputs": [],
            "epistemic": { "default_type": "hypothesis", "max_confidence": "hypothesis" },
            "known_limitations": "x",
            "verification_test": "x"
        });
        let failures = validate_gate1(&doc, ValidateGate1Options::default());
        assert!(failures.iter().any(|f| f.message.contains("Q-95")));
    }

    #[test]
    fn module_id_rule_matches_kernel_gate() {
        assert!(valid_module_id("my_gene_1"));
        assert!(!valid_module_id("My_gene"));
        assert!(!valid_module_id("x"));
    }
}
