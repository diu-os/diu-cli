# diu — command-line SDK for DIU OS genes

**Independent verification of computational and AI-generated scientific claims.**

A *gene* is a scientific model written as a passport, `gene.json`: the equation, its
source DOI, typed ports with physical units, the epistemic ceiling, known limitations
and one verification test. `diu` checks that passport with the same Gate 1 code the
DIU OS server runs (`diu-kernel`), so a local pass and a server pass cannot drift.

**License:** Apache-2.0. **Kernel:** [`diu-kernel`](https://github.com/diu-os/diu-kernel).

## Install

```bash
cargo install diu-cli
diu --version
```

## Use

```bash
diu new my_gene                 # scaffold my_gene/gene.json + README.md
diu test my_gene/gene.json      # kernel Gate 1, locally; exit code 0 = pass
diu import my_gene/gene.json    # candidate: Gate 1 + identity / revision / content_hash
diu mcp                         # MCP server on stdio for AI agents (see below)
DIU_TOKEN=… diu publish my_gene/gene.json --project <uuid>   # on a DIU OS instance
```

- `diu test` and `diu import` run offline. No token, no network.
- `content_hash` is RFC 8785 (JCS) → SHA-256 of the passport: key order does not matter.
- `diu publish` talks to `DIU_API` (default `https://diu-os.com`) and needs `DIU_TOKEN`.
  The server re-runs every gate and adds what a local tool cannot know: the DOI
  retraction list, the Gate 2 review and project ownership. A local pass is necessary,
  not sufficient.

## Use from an AI agent (MCP)

`diu mcp` is an [MCP](https://modelcontextprotocol.io) server on stdio. It is offline and
read-only: no network, no files, no token, no publish.

```bash
claude mcp add diu -- diu mcp                      # Claude Code
```

Claude Desktop, `claude_desktop_config.json`:

```json
{ "mcpServers": { "diu": { "command": "diu", "args": ["mcp"] } } }
```

| Tool | Gives back |
|---|---|
| `validate_gene` | Gate 1 PASS/FAIL with each issue, and identity / revision / content_hash. Same result as `diu import` |
| `digest` | RFC 8785 (JCS) → SHA-256 of any JSON |
| `evaluate_criterion` | One SVVRE outcome (`PASS` `PARTIAL` `FAIL` `INCONCLUSIVE` `NOT_APPLICABLE`) for a bar against a run's measurements, plus the bar's digest. Declare the bar before the run |
| `status_axis` | Which status vocabulary a word belongs to: declared ceiling, execution outcome or claim state. No conversion between them |

Every answer carries the K-4 line below.

## What a pass means

Gate 1 checks that the passport is **complete**, not that the model is right
(kernel invariant K-4: verification ≠ validation). `import_refused` is an operational
refusal, not a scientific verdict.

## Boundary

This tool and the kernel are open. The DIU OS platform (API, engines, frontend) is
the commercial service at [diu-os.com](https://diu-os.com) and is not in this tree.
