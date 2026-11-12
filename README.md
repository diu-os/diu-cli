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
DIU_TOKEN=… diu publish my_gene/gene.json --project <uuid>   # on a DIU OS instance
```

- `diu test` and `diu import` run offline. No token, no network.
- `content_hash` is RFC 8785 (JCS) → SHA-256 of the passport: key order does not matter.
- `diu publish` talks to `DIU_API` (default `https://diu-os.com`) and needs `DIU_TOKEN`.
  The server re-runs every gate and adds what a local tool cannot know: the DOI
  retraction list, the Gate 2 review and project ownership. A local pass is necessary,
  not sufficient.

## What a pass means

Gate 1 checks that the passport is **complete**, not that the model is right
(kernel invariant K-4: verification ≠ validation). `import_refused` is an operational
refusal, not a scientific verdict.

## Boundary

This tool and the kernel are open. The DIU OS platform (API, engines, frontend) is
the commercial service at [diu-os.com](https://diu-os.com) and is not in this tree.
