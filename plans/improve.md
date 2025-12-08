# Godo Improvement Plan

This document outlines a staged plan to improve the correctness, safety, functionality, and ergonomics of `godo`.

## Stage 1: Critical Correctness & Safety

These items address potential data corruption, race conditions, or broken environments.

- [x] **Fix Creation Race Condition**
    - **Issue:** The sandbox creation (worktree add + file cloning) is not guarded by a lock. Two concurrent `godo run` commands for the same new sandbox can race. Process A creates the worktree; Process B sees it as "existing" and starts running commands while Process A is still cloning files, leading to a broken state or errors.
    - **Fix:** Move the locking mechanism earlier. Acquire a named lock (e.g., on `.godo-leases/<sandbox>.lock` or a project-level lock) *before* checking for existence and creating/cloning the sandbox. The `SessionManager` should likely expose a `lock_sandbox(name)` method that returns a guard, to be held during the entire check-create-clone-run sequence, or at least during the check-create-clone phase.

- [ ] **Handle Submodules**
    - **Issue:** `godo` ignores `.git` at the root but `clonetree` recursively copies directories. If the repo contains submodules, `clonetree` might copy the submodule's `.git` file/dir. If it's a file (standard for submodules) pointing to an absolute path, it might work but points to the *original* repo's modules, risking corruption if the tool tries to modify them. If relative, it will break in the new location.
    - **Fix:** Explicitly handle submodules. Either:
        1.  Skip submodule directories during clone (mimicking a fresh clone without `git submodule update`).
        2.  Or, properly initialize submodules in the worktree using `git submodule update --init` (preferred for functionality, but slower).
        3.  At minimum, ensure `clonetree` excludes `.git` files recursively or `godo` scans and unlinks them to prevent confusing `git`.

- [x] **Protect Against "Dangling" Worktrees**
    - **Issue:** Git can retain a worktree entry even if the sandbox directory is missing (e.g., manual deletion or crash). `godo run` previously treated that state as live, leading to failures when commands attempted to run in a non-existent path.
    - **Fix:** Treat sandboxes as live only when the worktree directory exists; flag missing directories as "dangling" so they surface in `godo list` and are not reused. Added regression test to cover the case.

## Stage 2: Robustness & Logic

Improvements to make the tool more reliable in various edge cases.

- [ ] **Robust Integration Target Detection**
    - **Issue:** `find_integration_target` relies on heuristics (local main, upstream, etc.). It might fail or pick the wrong target in complex scenarios.
    - **Fix:** Add a configuration option (CLI flag `--base` or env var) to explicitly set the integration target branch. Fallback to the current heuristic if not set.

- [ ] **Symlink Handling**
    - **Issue:** Top-level symlinks are handled manually in `godo.rs` with `std::os::unix::fs::symlink`. Windows support is conditional but might be brittle.
    - **Fix:** Verify `clonetree` handles nested symlinks correctly. If not, implement a recursive walker that handles symlinks platform-agnostically (or consistently).

- [ ] **Exit Code Propagation**
    - **Issue:** `godo` propagates exit codes, but for signals (e.g., SIGINT/Ctrl+C), it might just exit with a code rather than killing the subprocess or handling the signal correctly.
    - **Fix:** Ensure signals are forwarded to the child process, or at least that the child process is terminated cleanly if `godo` is killed.

## Stage 3: Ergonomics & Features

Low-hanging fruit to make the tool nicer to use.

- [ ] **`godo status` Command**
    - **Idea:** Alias `godo list` to `godo status`, as it effectively shows the status of sandboxes.
    - **Benefit:** More intuitive for users looking for "what is happening".

- [ ] **Smart `clean` Defaults**
    - **Issue:** `godo clean` (no args) cleans *all* sandboxes. This might be destructive if the user didn't realize it.
    - **Fix:** Change `godo clean` (no args) to require confirmation "Are you sure you want to clean ALL sandboxes?" or default to current sandbox if inside one (though running from inside is currently blocked). Or require a `--all` flag for cleaning everything.

- [ ] **Shell Ergonomics**
    - **Issue:** `godo run --sh` requires quoting the command.
    - **Fix:** Detect if the command contains shell metacharacters and suggest `--sh` or auto-enable it (with a warning) if feasible? (Auto-enabling is risky, but a hint is good).

- [ ] **Gitignore Support**
    - **Idea:** Option to respect `.gitignore` during the clone.
    - **Benefit:** Sometimes you want a clean "build" sandbox, not a "dirty work-in-progress" sandbox.
    - **Implementation:** Use the `ignore` crate to walk the source dir instead of `fs::read_dir`.

## Stage 4: Code Quality

- [ ] **Test Coverage**
    - **Action:** Add unit tests for `SessionManager` race conditions (simulated). Add tests for submodule handling.
    - **Action:** Add property-based testing for path handling and names.

- [ ] **Dependency Review**
    - **Action:** Review `clonetree` path dependency. If it's stable, consider vendoring or publishing to lock it down.
    - **Action:** `sysinfo` is heavy. Ensure it's only used where strictly necessary (it is used for PID checks).
