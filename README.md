# pict-rs

A complete Rust port of the [PICT](https://github.com/microsoft/pict) combinatorial testing engine.

PICT (Pairwise Independent Combinatorial Testing) generates minimal test suites that cover all
pairwise — or N-way — combinations of parameter values. This crate reimplements the full C++
engine in safe Rust, exposing both a native Rust API and a C-compatible FFI layer that is a
drop-in replacement for the original `pictapi.h`.

## Features

- **Full algorithm parity** — MixedOrder, FixedOrder, Full, Flat, and Random generation modes
- **Exclusions** — forbid specific parameter-value combinations from appearing in output
- **Seeds** — force specific combinations to appear in the output
- **Weighted values** — influence value selection probability
- **Submodel hierarchies** — compose models for multi-table combinatorial designs
- **Implicit exclusion derivation** — automatically infers broader exclusions from narrower ones
- **C-compatible FFI** — `extern "C"` exports matching `pictapi.h` exactly

## Quick start

Add to `Cargo.toml`:

```toml
[dependencies]
pict = { path = "path/to/pict-rs" }
```

### Rust API

```rust
use pict::task::Task;
use pict::model::Model;
use pict::parameter::Parameter;
use pict::types::GenerationType;

// Three parameters: OS (3 values), Browser (3 values), RAM (2 values)
let mut task = Task::new();
let mut model = Model::new(String::new(), GenerationType::MixedOrder, 2, 0);

model.add_parameter(Parameter::new(2, 1, 3, "OS".into(), false));
model.add_parameter(Parameter::new(2, 2, 3, "Browser".into(), false));
model.add_parameter(Parameter::new(2, 3, 2, "RAM".into(), false));

task.set_root_model(Box::new(model));
task.generate().expect("generation failed");

for row in task.results().unwrap() {
    println!("{:?}", row);   // each entry is a 0-based value index
}
```

### C API (FFI)

The library exports the same symbols as the original `libpict`:

```c
#include "pictapi.h"         // original header still works

PICT_HANDLE task  = PictCreateTask();
PICT_HANDLE model = PictCreateModel(0);

PICT_HANDLE p1 = PictAddParameter(model, 3, PICT_PAIRWISE_GENERATION, NULL);
PICT_HANDLE p2 = PictAddParameter(model, 3, PICT_PAIRWISE_GENERATION, NULL);
PICT_HANDLE p3 = PictAddParameter(model, 2, PICT_PAIRWISE_GENERATION, NULL);

PictSetRootModel(task, model);
PictGenerate(task);

PICT_RESULT_ROW row = PictAllocateResultBuffer(task);
PictResetResultFetching(task);
while (PictGetNextResultRow(task, row)) {
    // row[0], row[1], row[2] are value indices
}
PictFreeResultBuffer(row);
PictDeleteTask(task);
PictDeleteModel(model);
```

## Building

```bash
cargo build --release
```

Produces `target/release/libpict.a` (static) and `target/release/libpict.so` (dynamic).

## Running tests

```bash
cargo test
```

## Architecture

```
src/
  lib.rs          C-compatible public API (extern "C" exports)
  types.rs        Enums, constants, and type aliases
  exclusion.rs    Exclusion and ExclusionCollection types
  trie.rs         Trie for prefix-containment lookup (used by deriver)
  parameter.rs    Parameter — values, weights, binding state
  combination.rs  Combination — bitvec, feasibility, apply_exclusion; bind_param/pick_value
  worklist.rs     WorkList — queue of parameters awaiting a value
  deriver.rs      ExclusionDeriver — implicit exclusion inference
  model.rs        Model — all five generation modes, gcd(), choose_and_link()
  task.rs         Task — orchestration, exclusion propagation, result access
  tests.rs        Unit tests
```

### Key design choices

| Topic | C++ | Rust |
|---|---|---|
| Parameter identity | Raw `Parameter*` pointer | Globally unique `u32` (`global_id`) |
| Global zeros counter | Field on `Model`, accessed via back-pointer from `Combination` | Local `i32` passed as `&mut` |
| Exclusion stability | `std::set` iterator stability | `HashMap<ExclusionId, Exclusion>` with stable IDs |
| Submodel params | `PseudoParameter` inheritance | `submodel_idx: Option<usize>` on `Parameter` |
| Random | `srand`/`rand` (global state) | Thread-local LCG — same POSIX interface |

## Relation to the original PICT

This is an independent Rust reimplementation. The combinatorial algorithm and all generation
modes are faithful ports of the C++17 original. Test output may differ in row ordering because
of RNG differences (the thread-local LCG is API-compatible with POSIX `rand()` but the glibc
implementation uses a more complex LFSR). Coverage guarantees are identical.

Original project: <https://github.com/microsoft/pict>  
License: MIT (same as original PICT)

## License

MIT
