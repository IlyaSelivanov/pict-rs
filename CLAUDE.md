# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build           # debug build
cargo build --release # release build (produces libpict.a + libpict.so/dll in target/release/)
cargo test            # run all tests
cargo test <name>     # run a single test by name
cargo clippy          # lint
```

## Project Overview

This is a pure-Rust port of Microsoft's PICT (Pairwise Independent Combinatorial Testing) engine. It produces a static lib, a C-compatible dynamic lib (FFI), and an rlib — no external crate dependencies. The crate (package) is named `pict-engine` (the original `pict-rs` name was taken on crates.io) but the compiled library itself is named `pict` (see `[lib] name` in `Cargo.toml`), matching the original C++ `libpict`.

## Architecture

### Layer separation

- **`lib.rs`** — C FFI boundary only. Exports `pictapi.h`-compatible symbols (`PictCreateTask`, `PictGenerate`, etc.) using raw pointers cast to/from `*mut c_void`. All unsafe code is confined here.
- **`task.rs`** — Orchestrates a full generation run: derives task-level exclusions, propagates them down the model hierarchy, drives `model.rs`, and owns the result buffer.
- **`model.rs`** — Core algorithm. Implements all five generation modes (MixedOrder, FixedOrder, Full, Flat, Random). The largest and most complex file (~1200 lines).
- **`combination.rs`** — Tracks N-way parameter-value combinations using bitvec-style state bytes (OPEN `0x00`, COVERED `0x01`, EXCLUDED `0xFF`) with mixed-radix encoding.
- **`deriver.rs`** — Infers implicit broader exclusions from narrower ones using a trie-based prefix-containment scan.
- **`exclusion.rs`** — Exclusion and ExclusionCollection types. Terms are kept sorted by `param_global_id` for stable lookup.
- **`parameter.rs`** — Parameter with values, weights, and binding state. Uses a global `AtomicU32` counter to assign stable IDs that survive Vec reallocations.
- **`trie.rs`** — Prefix-containment trie used by the deriver.
- **`worklist.rs`** — Simple `VecDeque`-based queue of parameters awaiting value assignment.
- **`types.rs`** — Enums (GenerationType, GenerationMode, ErrorType, ComboStatus) and type aliases (`ParamId = usize`, `CombId = usize`, `ExclusionId = u32`).
- **`tests.rs`** — Unit tests exercising the Rust API directly (not through FFI).

### Identity strategy

Parameters and exclusions get unique IDs from `AtomicU32` global counters at creation time. These IDs remain stable across collection mutations and are used as the FFI handle values (cast to `*mut c_void` in `lib.rs`).

### Exclusion flow

1. Caller adds exclusions via `PictAddExclusion` → stored in the Task.
2. At `PictGenerate` time, `task.rs` runs `ExclusionDeriver` to infer broader exclusions, then distributes the full set to the root model and each submodel.
3. `combination.rs` marks excluded combinations as `0xFF` so the algorithm skips them.

### Submodel hierarchy

`PictAttachChildModel` links models into a tree. The parent model treats a child's output row as a single "parameter" during its own generation pass, enabling hierarchical combinatorial coverage.
