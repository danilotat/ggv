# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

GGV is a genome viewer (aligned reads, variants, gene annotations, signal tracks) with three front ends over one shared implementation: `ggv-desktop` (egui + wgpu app), `ggv` (CLI), and `ggv mcp` (MCP server). A core design invariant: a query at the same locus must return the same numbers from all three, so logic belongs in the shared library crates, not in a front end. GGV displays and counts what is in the input files; it does not call variants.

## Commands

Rust 1.91+, edition 2024. Workspace `unsafe_code = "forbid"` (the desktop crate denies it, with one audited exception for the Windows console attach in `main.rs`).

```sh
cargo build --release -p ggv-cli -p ggv-desktop    # -> target/release/ggv, ggv-desktop
cargo build --workspace --all-targets --locked      # what CI runs
cargo test --workspace --locked                     # what CI runs (CI also runs a CLI + MCP smoke test, see ci.yml)
cargo test -p ggv-layout --test long_reads          # one integration test file
cargo test -p ggv-cli mcp::                         # one module's unit tests (tests are mostly in-crate `#[cfg(test)]`)
cargo run -p ggv-cli -- query pileup tests/data/hg002-revio/HG002.revio.slices.bam chr20:<pos> -f tests/data/hg002-revio/GRCh38.chr20.slices.fa.gz
```

- On Linux the desktop crate needs `libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libwayland-dev`.
- `pasta` (the SAM/BAM/CRAM/VCF/FASTA parser, by the same author) is a git dependency pinned by `rev` in the root `Cargo.toml`, not on crates.io. `.cargo/config.toml` makes cargo fetch git deps with the git CLI. To bump pasta, change `rev` and `version`.
- On Windows, if linking hits `LNK1104` (path too long), use `CARGO_TARGET_DIR=C:/t`.
- Large-data tests skip themselves unless env vars are set (`GGV_TEST_BAM`, `GGV_TEST_REGIONS`, optionally `GGV_TEST_REF`; `GGV_TEST_WGS_BAM`/`GGV_TEST_WGS_REGIONS`). Small real fixtures live in `tests/data/` (`hg002-revio` is a 4 MB PacBio HiFi slice with matching reference, VCFs and GTF, used by the long-read checks, and runs everywhere; `fixtures/` holds small VCF/signal files).
- `scripts/compare_mpileup.py` cross-checks `ggv query pileup` against `samtools mpileup -A -B -Q 0 -q 0 -d 0 -x --reverse-del` position by position. Note `-A`: GGV, like IGV, shows anomalous pairs that samtools drops by default.
- Desktop scripted runs (no interaction, then exit): `ggv-desktop --screenshot out.png`, `--bench`, `--zoom-cycle`, `--demo`. Failures set exit status 1 (`automation::fail`).
- Version is a single `[workspace.package] version`; `scripts/build_identity.rs` is the `build.rs` for both binaries and stamps commit/target into `--version`, About and bug reports. Release packaging is under `scripts/{windows,linux,macos}`.

## Architecture

Crate layering (dependencies point downward):

```
ggv-model    Loci/regions/contigs; no I/O (kept WASM-buildable)
ggv-io       Indexed BAM/CRAM/FASTA/2bit/VCF/BCF/bigWig via pasta; locus resolution (chr7<->7, chrM<->MT);
             built-in genomes + demo download/cache; the attach protocol (attach.rs)
ggv-query    pileup (per position) and summarize (per region) engines, no rendering
ggv-annot    GTF/GFF3/BED transcripts, interval index, gene search, coding.rs (HGVS consequences), MANE table
ggv-layout   Decodes reads (blocks, indels, mismatches), packs rows, bins coverage, phasing, base mods
ggv-render   Backend-neutral `Scene` -> PNG (tiny-skia, built-in font) or SVG
ggv-cli      `ggv` binary: query / render / batch / index / mcp
ggv-desktop  egui + wgpu app (GPU-instanced reads; panning/zooming only change a uniform)
```

Things that need several files to understand:

- **Single source of truth for numbers.** Coverage drawn from decoded reads (`ggv-layout`, used by `render` and the app) must equal the pileup engine (`ggv-query`) at every position; `crates/ggv-layout/tests/coverage_parity.rs` enforces this. Filters (`mapq >= 20 && hp == 1 && !dup`) are one expression language applied identically to every count in all three front ends.
- **MCP server** (`crates/ggv-cli/src/mcp/`): hand-rolled newline-delimited JSON-RPC 2.0 on stdio (only `initialize`, `tools/list`, `tools/call`, `ping`), no MCP SDK. Stdout is protocol only; log to stderr. Input is read on its own thread so `ping`/cancellation work during long calls; other requests run in order. `tools.rs` holds every tool (open files are cached by path, annotations parsed once); `validate.rs` checks arguments; `desktop.rs` is the attached mode. Tool results are compact JSON summaries for a model (raw reads only when asked, capped), each carrying a `query` block of the files/filters/region used.
- **Attach mode** (`ggv mcp --attach`): each desktop window listens on loopback TCP and writes `<config>/ggv/attach/<pid>.json` (port + random token; `GGV_ATTACH_DIR` overrides the folder for tests). Protocol is newline-delimited JSON with a per-request token. Protocol/endpoint code is `ggv-io/src/attach.rs`; the window's side is `ggv-desktop/src/attach.rs`. `state` is answered from a per-frame snapshot (never waits for a draw); view-changing commands (`goto`/`select`/`open`...) are queued and executed on the window's next frame, with an abandon/take atomic so a timed-out command never runs late. User actions are logged as events that `events` waits on.
- **Desktop app**: `main.rs` is CLI parsing/startup; `app.rs` (very large) plus `app/*.rs` (actions, control, tools, palette, workspace, demo, ...) is the egui UI; `loader.rs` loads data on a background thread a few view widths at a time; `tracks.rs`/`tracks.wgsl` is the GPU path; `automation.rs` is the scripted screenshot/bench harness; `review.rs` is variant triage (labels saved next to the VCF); `session.rs`/`store.rs` persist state.
- **Memory:** desktop uses mimalloc to return freed memory to the OS (see `main.rs`).

User-facing docs are in `docs/` (`desktop.md`, `cli.md`, `mcp.md`, `genomes.md`, `install.md`, `development.md`); update `docs/mcp.md` / `docs/cli.md` when tools or options change. CI (`.github/workflows/ci.yml`) builds and tests on Linux, Windows and macOS.
