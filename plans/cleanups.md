# Code Cleanup and Refactoring Plan

This plan aims to streamline the `godo` codebase by reducing the complexity of large "God objects" (specifically `Godo` struct and `main.rs`) and improving the project structure. The focus is on the "rcli" (Rust CLI) and "database" (Metadata) components.

## Phase 1: `libgodo` Core Refactoring

The `crates/libgodo/src/godo.rs` file is over 1600 lines and handles too many responsibilities. We will split it into focused modules.

### 1.1 Extract Errors
**Goal**: Move `GodoError` and related logic out of `godo.rs`.
- **Action**: Create `crates/libgodo/src/error.rs`.
- **Benefit**: Centralized error definitions, declutters core logic.

### 1.2 Extract Domain Types
**Goal**: Move data structures (`SandboxStatus`, `PrepareSandboxPlan`, `SandboxSession`, `RemovalPlan`, etc.) out of `godo.rs`.
- **Action**: Create `crates/libgodo/src/types.rs`.
- **Benefit**: clearer API surface, easier to read.

### 1.3 Modularize `Godo` Implementation
**Goal**: Split the massive `impl Godo` block.
- **Action**: Keep `Godo` struct in `godo.rs` (or rename to `lib.rs` re-export), but move complex logic into internal helpers or use extension traits/sub-modules.
- **Sub-tasks**:
    - Move "Project Name" and "Branch Name" logic to `crates/libgodo/src/naming.rs` (or `utils.rs`).
    - Move `SandboxMetadataStore` fully to `crates/libgodo/src/store.rs` (rename from `metadata.rs` to clarify "database" role).

## Phase 2: `godo` CLI Refactoring

The `crates/godo/src/main.rs` file is over 1100 lines and mixes argument parsing, UI logic, and business logic.

### 2.1 Extract Argument Parsing
**Goal**: Separate `clap` definitions from runtime logic.
- **Action**: Create `crates/godo/src/args.rs` containing `Cli`, `Commands`, and `RunRequest` structs.
- **Benefit**: `main.rs` becomes focused on execution flow.

### 2.2 Modularize Command Handlers
**Goal**: Move command-specific logic (`run_command`, `list_command`, etc.) into their own modules.
- **Action**: Create `crates/godo/src/commands/` directory.
    - `crates/godo/src/commands/mod.rs`
    - `crates/godo/src/commands/run.rs`
    - `crates/godo/src/commands/list.rs`
    - `crates/godo/src/commands/diff.rs`
    - `crates/godo/src/commands/remove.rs`
    - `crates/godo/src/commands/clean.rs`
- **Benefit**: massive reduction in `main.rs` size, easier to test and maintain individual commands.

## Phase 3: Infrastructure & Utils

### 3.1 Unify Utilities
**Goal**: Check for duplicated code between `git.rs` and other modules.
- **Action**: Review `git.rs` for overly specific functions that should be generic, or vice versa.

### 3.2 "Database" Improvements
**Goal**: Strengthen the metadata storage.
- **Action**: Ensure `store.rs` (formerly `metadata.rs`) has clear atomic read/write semantics and proper error wrapping (already mostly there, but can be cleaned up).

## Execution Order

1.  `libgodo`: Extract `error.rs` and `types.rs`.
2.  `libgodo`: Refactor `metadata.rs` -> `store.rs`.
3.  `godo` (CLI): Extract `args.rs`.
4.  `godo` (CLI): Extract command handlers to `commands/` submodule.

## Verification
- Run `cargo check` after every file move.
- Run `cargo test` to ensure no regressions.
