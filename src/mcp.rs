// `diu mcp` — MCP server over stdio (ADR D-139).
//
// JSON-RPC 2.0, one message per line on stdin, one response per line on
// stdout. Logs go to stderr only: stdout belongs to the protocol.
//
// Deliberately synchronous: four offline, read-only tools need no async
// runtime, so D-078 items 1–4 (select!/timeout, async mutexes, child
// processes, SQL transactions) do not arise. There is no network, no file
// access, no environment read and no `publish` here (D-139 decision 4).
//
// Every tool answer carries the K-4 line: the kernel reports the shape of a
// passport or a result, never the adequacy of a model to the world.

use crate::evaluate_import;
use diu_kernel::criterion::{evaluate, CriterionSet};
use diu_kernel::digest::canonical_digest;
use diu_kernel::vocabulary::{axis_of, Axis};
use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};

/// Per-message input bound (D-139 decision 4). Exceeding it is a JSON-RPC
/// error, not a crash and not an unbounded allocation.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// Protocol revisions this server speaks, newest first. On `initialize` the
/// server echoes the client's revision if it is listed, otherwise offers the
/// newest (MCP lifecycle, version negotiation).
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const K4: &str =
    "K-4: the kernel checks the shape of a passport or a result against declared rules. \
A PASS is not a statement that the model is right about the world.";

const INSTRUCTIONS: &str = "DIU OS kernel tools, offline and read-only. \
validate_gene runs Gate 1 on a gene passport (gene.json) and returns its identity/revision/content_hash; \
digest computes the RFC 8785 JCS → SHA-256 hash of any JSON; \
evaluate_criterion turns a bar declared before a run plus the run's measurements into one SVVRE outcome; \
status_axis says which status vocabulary a word belongs to. \
Nothing here publishes, contacts a server, or reads files. ";

// JSON-RPC 2.0 error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Run the server until stdin closes.
pub fn run() -> std::process::ExitCode {
    eprintln!(
        "diu mcp {} (diu-kernel {}) — stdio, offline, read-only",
        env!("CARGO_PKG_VERSION"),
        diu_kernel::VERSION
    );
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match serve(stdin.lock(), stdout.lock()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("diu mcp: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Read messages from `input` and write responses to `output` until EOF.
pub fn serve<R: BufRead, W: Write>(mut input: R, mut output: W) -> std::io::Result<()> {
    loop {
        let reply = match read_bounded_line(&mut input, MAX_MESSAGE_BYTES)? {
            Line::Eof => return Ok(()),
            Line::TooLarge => Some(error(
                Value::Null,
                INVALID_REQUEST,
                &format!("message exceeds {MAX_MESSAGE_BYTES} bytes"),
            )),
            Line::Message(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                let text = text.trim();
                if text.is_empty() {
                    None
                } else {
                    handle_message(text)
                }
            }
        };
        if let Some(reply) = reply {
            serde_json::to_writer(&mut output, &reply)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

enum Line {
    Message(Vec<u8>),
    TooLarge,
    Eof,
}

/// Read one `\n`-terminated line, keeping at most `limit` bytes in memory. An
/// oversized line is drained to its end and reported, so the stream stays in
/// sync for the next message.
fn read_bounded_line<R: BufRead>(input: &mut R, limit: usize) -> std::io::Result<Line> {
    let mut buf = Vec::new();
    let mut too_large = false;
    let mut saw_any = false;
    loop {
        let chunk = input.fill_buf()?;
        if chunk.is_empty() {
            return Ok(match (saw_any, too_large) {
                (false, _) => Line::Eof,
                (true, true) => Line::TooLarge,
                (true, false) => Line::Message(buf),
            });
        }
        saw_any = true;
        let (take, done) = match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (chunk.len(), false),
        };
        if !too_large {
            if buf.len() + take > limit + 1 {
                too_large = true;
                buf = Vec::new();
            } else {
                buf.extend_from_slice(&chunk[..take]);
            }
        }
        input.consume(take);
        if done {
            return Ok(if too_large {
                Line::TooLarge
            } else {
                Line::Message(buf)
            });
        }
    }
}

/// Handle one raw message. `None` means no response is due (a notification).
pub fn handle_message(text: &str) -> Option<Value> {
    let msg: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return Some(error(
                Value::Null,
                PARSE_ERROR,
                &format!("parse error: {e}"),
            ))
        }
    };
    let Some(obj) = msg.as_object() else {
        // Batches were removed from MCP in 2025-06-18; a bare array or scalar
        // is not a request.
        return Some(error(
            Value::Null,
            INVALID_REQUEST,
            "expected a single JSON-RPC request object",
        ));
    };
    let id = obj.get("id").cloned();
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        // A response or a malformed request. Responses from the client are
        // not expected by this server and are ignored.
        return id
            .filter(|_| !obj.contains_key("result") && !obj.contains_key("error"))
            .map(|id| error(id, INVALID_REQUEST, "missing method"));
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);

    // Notifications carry no id and get no response, known or not.
    let id = id?;

    let outcome = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(&params),
        other => Err((METHOD_NOT_FOUND, format!("method not found: {other}"))),
    };
    Some(match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error(id, code, &message),
    })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|v| PROTOCOL_VERSIONS.contains(v))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "diu",
            "title": "DIU OS kernel",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": format!("{INSTRUCTIONS}{K4}")
    })
}

fn tool_definitions() -> Value {
    let read_only = json!({ "readOnlyHint": true, "openWorldHint": false, "idempotentHint": true });
    json!([
        {
            "name": "validate_gene",
            "title": "Validate a gene passport (Gate 1)",
            "description": "Run the DIU OS kernel's Gate 1 on a gene passport (the contents of gene.json) — the same validator the server runs on publish. Returns PASS/FAIL with each issue, and the INV-039 triple: identity (module_id), revision (version), content_hash (RFC 8785 JCS → SHA-256 of the passport). Server-only checks (DOI retraction Q-96, Gate 2 review, ownership) are not run here. PASS means the passport is complete, not that the model is right.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "gene": { "type": "object", "description": "The gene passport (gene.json) as a JSON object." },
                    "tier": { "type": "integer", "minimum": 0, "maximum": 2, "default": 2, "description": "Subscription tier the gate is evaluated for (0, 1 or 2)." }
                },
                "required": ["gene"],
                "additionalProperties": false
            },
            "annotations": read_only
        },
        {
            "name": "digest",
            "title": "Canonical hash of JSON",
            "description": "RFC 8785 (JCS) canonicalisation, then SHA-256, as lowercase hex — the one digest definition DIU OS uses for results, manifests and criteria. Key order and whitespace do not change it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "value": { "description": "Any JSON value." }
                },
                "required": ["value"],
                "additionalProperties": false
            },
            "annotations": read_only
        },
        {
            "name": "evaluate_criterion",
            "title": "Evaluate a declared bar (SVVRE)",
            "description": "Evaluate an acceptance criterion against a run's measurements and return one SVVRE outcome: PASS, PARTIAL, FAIL, INCONCLUSIVE or NOT_APPLICABLE, plus the criterion's digest. Declare the criterion BEFORE the run and keep its digest: a bar chosen after seeing the result is not a test (Р-3, K-6/K-7). A missing metric is NOT_APPLICABLE, not FAIL; a non-numeric one is INCONCLUSIVE.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "criterion": {
                        "type": "object",
                        "description": "CriterionSet: { \"all\": [ { \"metric\": \"fit.r_squared\", \"op\": \"ge\", \"threshold\": 0.9 }, ... ] }. op ∈ lt, le, gt, ge, within (within requires tolerance). metric is a dot path into measurements.",
                        "properties": {
                            "all": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "metric": { "type": "string" },
                                        "op": { "type": "string", "enum": ["lt", "le", "gt", "ge", "within"] },
                                        "threshold": { "type": "number" },
                                        "tolerance": { "type": "number", "minimum": 0 }
                                    },
                                    "required": ["metric", "op", "threshold"]
                                }
                            }
                        }
                    },
                    "measurements": { "type": "object", "description": "The run's canonical result as a JSON object." }
                },
                "required": ["criterion", "measurements"],
                "additionalProperties": false
            },
            "annotations": read_only
        },
        {
            "name": "status_axis",
            "title": "Which axis a status word belongs to",
            "description": "Say which of DIU OS's three status vocabularies a literal belongs to (D-137): declared ceiling (fact, hypothesis, speculation, unknown), execution outcome (SVVRE), or claim state (ten states such as SUPPORTED, CONTESTED). Case-sensitive. There is no conversion between axes: PASS does not become SUPPORTED.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "literal": { "type": "string", "description": "The status word, e.g. \"hypothesis\", \"PASS\", \"CONTESTED\"." }
                },
                "required": ["literal"],
                "additionalProperties": false
            },
            "annotations": read_only
        }
    ])
}

/// `tools/call`. An unknown tool is a protocol error; bad arguments to a known
/// tool are a tool error (`isError: true`) so the agent can read and correct
/// them (MCP tools spec).
fn call_tool(params: &Value) -> Result<Value, (i64, String)> {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err((INVALID_PARAMS, "tools/call requires `name`".into()));
    };
    let empty = Map::new();
    let args = match params.get("arguments") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(m)) => m,
        Some(_) => return Ok(tool_error("`arguments` must be an object")),
    };
    let result = match name {
        "validate_gene" => tool_validate_gene(args),
        "digest" => tool_digest(args),
        "evaluate_criterion" => tool_evaluate_criterion(args),
        "status_axis" => tool_status_axis(args),
        other => return Err((INVALID_PARAMS, format!("unknown tool: {other}"))),
    };
    Ok(match result {
        Ok(structured) => tool_ok(structured),
        Err(message) => tool_error(&message),
    })
}

fn tool_ok(mut structured: Value) -> Value {
    if let Some(obj) = structured.as_object_mut() {
        obj.insert("note".into(), json!(K4));
    }
    let text = serde_json::to_string_pretty(&structured).unwrap_or_default();
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    })
}

fn tool_error(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": format!("{message}\n{K4}") }],
        "isError": true
    })
}

fn reject_unknown(args: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    match args.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!(
            "unknown argument `{k}`; expected {}",
            allowed.join(", ")
        )),
        None => Ok(()),
    }
}

fn tool_validate_gene(args: &Map<String, Value>) -> Result<Value, String> {
    reject_unknown(args, &["gene", "tier"])?;
    let gene = match args.get("gene") {
        Some(g @ Value::Object(_)) => g,
        Some(_) => return Err("`gene` must be a JSON object (the contents of gene.json)".into()),
        None => return Err("missing required argument `gene`".into()),
    };
    let tier = match args.get("tier") {
        None | Some(Value::Null) => 2,
        Some(v) => match v.as_i64() {
            Some(t @ 0..=2) => t as i32,
            _ => return Err("`tier` must be an integer 0, 1 or 2".into()),
        },
    };
    let verdict = evaluate_import(gene, tier);
    let content_hash = verdict.content_hash.map_err(|e| format!("digest: {e}"))?;
    Ok(json!({
        "gate1": if verdict.passed { "PASS" } else { "FAIL" },
        "tier": tier,
        "failures": verdict
            .failures
            .iter()
            .map(|f| json!({ "block_id": f.block_id, "message": f.message }))
            .collect::<Vec<_>>(),
        "identity": verdict.module_id,
        "revision": verdict.version,
        "content_hash": content_hash,
        "candidate": "candidate ≠ published: nothing was registered or uploaded",
        "not_checked_here": [
            "DOI retraction list (Q-96) — server-side",
            "Gate 2 review — server-side",
            "project ownership — server-side"
        ]
    }))
}

fn tool_digest(args: &Map<String, Value>) -> Result<Value, String> {
    reject_unknown(args, &["value"])?;
    let Some(value) = args.get("value") else {
        return Err("missing required argument `value`".into());
    };
    let digest = canonical_digest(value).map_err(|e| format!("digest: {e}"))?;
    Ok(json!({
        "digest": digest,
        "algorithm": "RFC 8785 JCS → SHA-256, lowercase hex"
    }))
}

fn tool_evaluate_criterion(args: &Map<String, Value>) -> Result<Value, String> {
    reject_unknown(args, &["criterion", "measurements"])?;
    let criterion = args
        .get("criterion")
        .ok_or("missing required argument `criterion`")?;
    let measurements = match args.get("measurements") {
        Some(m @ Value::Object(_)) => m,
        Some(_) => return Err("`measurements` must be a JSON object".into()),
        None => return Err("missing required argument `measurements`".into()),
    };
    let set: CriterionSet = serde_json::from_value(criterion.clone())
        .map_err(|e| format!("`criterion` is not a CriterionSet: {e}"))?;
    set.validate()
        .map_err(|e| format!("criterion refused at declaration: {e}"))?;
    let digest = set.digest().map_err(|e| format!("digest: {e}"))?;
    let outcome = evaluate(&set, measurements);
    Ok(json!({
        "outcome": outcome.as_str(),
        "criterion_digest": digest,
        "predicates": set.all.len(),
        "declare_first": "The bar counts only if it was declared before the run. Record criterion_digest before running; this tool cannot see when you chose it (Р-3, K-6/K-7)."
    }))
}

fn axis_name(axis: Axis) -> &'static str {
    match axis {
        Axis::DeclaredCeiling => "declared_ceiling",
        Axis::ExecutionOutcome => "execution_outcome",
        Axis::ClaimState => "claim_state",
    }
}

fn axis_question(axis: Axis) -> &'static str {
    match axis {
        Axis::DeclaredCeiling => "How strong may a statement built on this be, at most?",
        Axis::ExecutionOutcome => {
            "Did this execution clear the bar its experiment declared first?"
        }
        Axis::ClaimState => "What has a human assessment of the evidence established about this claim version, in this scope?",
    }
}

fn tool_status_axis(args: &Map<String, Value>) -> Result<Value, String> {
    reject_unknown(args, &["literal"])?;
    let literal = match args.get("literal") {
        Some(Value::String(s)) => s.as_str(),
        Some(_) => return Err("`literal` must be a string".into()),
        None => return Err("missing required argument `literal`".into()),
    };
    let rule = "No conversion between axes (D-137 R-2): hypothesis does not become UNTESTED, PASS does not become SUPPORTED. Crossing an axis is a human assessment, never a lookup.";
    Ok(match axis_of(literal) {
        Some(axis) => json!({
            "literal": literal,
            "canonical": true,
            "axis": axis_name(axis),
            "question": axis_question(axis),
            "carried_by": axis.carried_by(),
            "axis_literals": axis.literals(),
            "rule": rule
        }),
        None => {
            let near = Axis::ALL
                .into_iter()
                .flat_map(|a| a.literals().iter().copied())
                .find(|l| l.eq_ignore_ascii_case(literal));
            json!({
                "literal": literal,
                "canonical": false,
                "message": "Not a canonical literal on any axis. Literals are case-sensitive.",
                "did_you_mean": near,
                "rule": rule
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn req(id: i64, method: &str, params: Value) -> String {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
    }

    fn call(name: &str, arguments: Value) -> Value {
        let line = req(
            1,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        handle_message(&line).expect("response")["result"].clone()
    }

    fn run_lines(input: &str) -> Vec<Value> {
        let mut out = Vec::new();
        serve(Cursor::new(input.as_bytes().to_vec()), &mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn initialize_negotiates_version() {
        let r = handle_message(&req(1, "initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } }))).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "diu");
        assert!(r["result"]["capabilities"]["tools"].is_object());

        let r = handle_message(&req(
            2,
            "initialize",
            json!({ "protocolVersion": "1999-01-01" }),
        ))
        .unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn full_session_over_stdio() {
        let input = [
            req(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
            req(2, "tools/list", json!({})),
            req(3, "ping", json!({})),
            String::new(),
        ]
        .join("\n");
        let out = run_lines(&input);
        // The notification gets no response; the blank line is skipped.
        assert_eq!(out.len(), 3);
        assert_eq!(out[0]["id"], 1);
        let names: Vec<_> = out[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "validate_gene",
                "digest",
                "evaluate_criterion",
                "status_axis"
            ]
        );
        assert_eq!(out[2]["result"], json!({}));
    }

    #[test]
    fn unknown_method_and_unknown_tool_are_protocol_errors() {
        let r = handle_message(&req(1, "resources/list", json!({}))).unwrap();
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        let r = handle_message(&req(2, "tools/call", json!({ "name": "publish" }))).unwrap();
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        // Unknown notifications are silently ignored.
        assert!(handle_message(r#"{"jsonrpc":"2.0","method":"notifications/whatever"}"#).is_none());
    }

    #[test]
    fn malformed_input_is_an_error_not_a_crash() {
        let r = handle_message("{not json").unwrap();
        assert_eq!(r["error"]["code"], PARSE_ERROR);
        assert_eq!(r["id"], Value::Null);
        let r = handle_message("[]").unwrap();
        assert_eq!(r["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn bad_arguments_are_tool_errors() {
        let r = call("validate_gene", json!({}));
        assert_eq!(r["isError"], true);
        let r = call("validate_gene", json!({ "gene": "x" }));
        assert_eq!(r["isError"], true);
        let r = call("digest", json!({ "value": 1, "extra": 2 }));
        assert_eq!(r["isError"], true);
        let r = call("status_axis", json!({ "literal": 5 }));
        assert_eq!(r["isError"], true);
        let r = call(
            "evaluate_criterion",
            json!({ "criterion": { "all": [{ "metric": "x", "op": "within", "threshold": 1.0 }] }, "measurements": {} }),
        );
        assert_eq!(r["isError"], true);
        assert!(r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("tolerance"));
    }

    #[test]
    fn oversized_message_is_refused_and_stream_stays_in_sync() {
        let big = format!("{{\"pad\":\"{}\"}}", "a".repeat(MAX_MESSAGE_BYTES + 10));
        let input = format!("{big}\n{}\n", req(7, "ping", json!({})));
        let out = run_lines(&input);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["error"]["code"], INVALID_REQUEST);
        assert_eq!(out[1]["id"], 7);
    }

    #[test]
    fn validate_gene_matches_diu_import() {
        let gene = crate::tests::valid_import_doc();
        let r = call("validate_gene", json!({ "gene": gene.clone() }));
        let s = &r["structuredContent"];
        let v = evaluate_import(&gene, 2);
        assert_eq!(s["gate1"], "PASS");
        assert_eq!(s["identity"], v.module_id.as_str());
        assert_eq!(s["revision"], v.version.as_str());
        assert_eq!(s["content_hash"], v.content_hash.unwrap().as_str());
        assert_eq!(s["note"], K4);

        let mut bad = gene;
        bad["mathematical_model"]["source_doi"] = json!("");
        let r = call("validate_gene", json!({ "gene": bad.clone() }));
        let s = &r["structuredContent"];
        assert_eq!(s["gate1"], "FAIL");
        let want: Vec<_> = evaluate_import(&bad, 2)
            .failures
            .iter()
            .map(|f| f.message.clone())
            .collect();
        let got: Vec<_> = s["failures"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["message"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(got, want);
        assert_eq!(
            r["isError"], false,
            "a FAIL verdict is an answer, not a tool error"
        );
    }

    #[test]
    fn digest_matches_kernel_and_ignores_key_order() {
        let a = json!({ "b": 1, "a": [1.5, "x"] });
        let b: Value = serde_json::from_str(r#"{"a":[1.5,"x"],"b":1}"#).unwrap();
        let ra = call("digest", json!({ "value": a.clone() }));
        let rb = call("digest", json!({ "value": b }));
        assert_eq!(
            ra["structuredContent"]["digest"],
            canonical_digest(&a).unwrap().as_str()
        );
        assert_eq!(
            ra["structuredContent"]["digest"],
            rb["structuredContent"]["digest"]
        );
    }

    #[test]
    fn evaluate_criterion_gives_svvre_and_digest() {
        let criterion = json!({ "all": [
            { "metric": "fit.r_squared", "op": "ge", "threshold": 0.9 },
            { "metric": "peak", "op": "within", "threshold": 10.0, "tolerance": 0.5 }
        ] });
        let set: CriterionSet = serde_json::from_value(criterion.clone()).unwrap();
        let cases = [
            (
                json!({ "fit": { "r_squared": 0.95 }, "peak": 10.2 }),
                "PASS",
            ),
            (
                json!({ "fit": { "r_squared": 0.95 }, "peak": 12.0 }),
                "PARTIAL",
            ),
            (json!({ "fit": { "r_squared": 0.1 }, "peak": 12.0 }), "FAIL"),
            (
                json!({ "fit": { "r_squared": "n/a" }, "peak": 10.0 }),
                "INCONCLUSIVE",
            ),
            (json!({ "other": 1 }), "NOT_APPLICABLE"),
        ];
        for (m, want) in cases {
            let r = call(
                "evaluate_criterion",
                json!({ "criterion": criterion, "measurements": m }),
            );
            assert_eq!(r["structuredContent"]["outcome"], want, "{m}");
            assert_eq!(
                r["structuredContent"]["criterion_digest"],
                set.digest().unwrap().as_str()
            );
        }
    }

    #[test]
    fn status_axis_names_one_axis_and_refuses_conversion() {
        for (lit, axis) in [
            ("hypothesis", "declared_ceiling"),
            ("PASS", "execution_outcome"),
            ("CONTESTED", "claim_state"),
        ] {
            let s = call("status_axis", json!({ "literal": lit }))["structuredContent"].clone();
            assert_eq!(s["axis"], axis);
            assert_eq!(s["canonical"], true);
        }
        let s = call("status_axis", json!({ "literal": "pass" }))["structuredContent"].clone();
        assert_eq!(s["canonical"], false);
        assert_eq!(s["did_you_mean"], "PASS");
        let s = call("status_axis", json!({ "literal": "VERIFIED" }))["structuredContent"].clone();
        assert_eq!(s["canonical"], false);
        assert_eq!(s["did_you_mean"], Value::Null);
    }
}
