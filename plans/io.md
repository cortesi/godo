# Libgodo I/O Extraction Plan

Shift all user I/O out of libgodo so it is a clean, reusable core library, while the CLI (or a
new godo-term layer) owns all prompts, spinners, and terminal rendering. Redesign the libgodo
API to be GUI-friendly and explicit about decisions.

1. Stage One: Inventory and API review

Audit all user-facing I/O and decision points, then capture the current API surface to guide
redesign.

1. [x] Scan libgodo for terminal I/O, prompts, spinners, and stdin/stdout inheritance
   (e.g., `output.rs`, `Stdio::inherit` call sites, examples/tests).
2. [x] Run `ruskel` on libgodo to capture the current public API and identify I/O exposure.
3. [x] Map each prompt/spinner/output usage to a proposed replacement: explicit options, a
   returned decision enum, or an event/progress callback.
4. [x] Decide the I/O home: move terminal I/O into `crates/godo` or create a `godo-term`
   module/crate, and note the dependency changes needed.

2. Stage Two: Redesign libgodo APIs

Remove all direct user I/O from libgodo and make decision points explicit to callers.

1. [x] Replace `Output` usage in libgodo with data return types and/or explicit option structs
   (e.g., `RunOptions`, `RemoveOptions`) that capture decisions upfront.
2. [x] Introduce structured results for operations that currently prompt mid-flow so callers can
   decide and resume (e.g., `RunPlan`, `RemovalPlan`, `SandboxStatus`).
3. [x] Remove `output` module from libgodo and update `lib.rs` exports and docs accordingly.
4. [x] Update libgodo error types to drop `OutputError` and keep errors strictly core-domain.
5. [x] Update libgodo tests/examples to use the new APIs without terminal mocks.

3. Stage Three: Re-home terminal I/O

Implement CLI-facing I/O in godo (or godo-term) and wire it to the new libgodo APIs.

1. [x] Move terminal rendering, prompts, and spinner implementations to godo or a new
   godo-term layer; update Cargo dependencies (remove from libgodo, add where needed).
2. [x] Update godo CLI command handlers to drive the new decision-based libgodo APIs and
   render all user messaging locally.
3. [x] Add any new UI-facing abstractions needed for CLI reuse (e.g., Output + Terminal types)
   without leaking them back into libgodo.

4. Stage Four: Validation and cleanup

Ensure the refactor is clean, tested, and documented.

1. [x] Run `cargo clippy -q --fix --all --all-targets --all-features --allow-dirty --tests \
   --examples` and fix any warnings.
2. [x] Run `cargo test --all`.
3. [x] Format with `cargo +nightly fmt --all -- --config-path ./rustfmt-nightly.toml` (or
   `cargo +nightly fmt --all`).
4. [x] Update README or crate docs to describe the new libgodo API expectations and the I/O
   layering.
