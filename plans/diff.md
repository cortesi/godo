# godo diff SANDBOX

This plan designs a godo command that diffs a sandbox against its base commit, including staged,
unstaged, and untracked changes. It adds an optional base override and pager configuration, then
validates behavior with tests and docs updates. The scope is limited to git-backed sandboxes;
non-git sandboxes return a clear, consistent error.

1. Stage One: Clarify UX and semantics
1. [x] Define base commit semantics: prefer the recorded HEAD at sandbox creation; if missing, use
    the merge-base of the sandbox branch and origin/main; error if neither can be resolved.
2. [x] Specify metadata-missing behavior: fail hard by default; allow explicit fallback only with a
    warning message when used.
3. [x] Define untracked inclusion: include by default and render as `git diff --no-index /dev/null
    <file>`.
4. [x] Finalize CLI surface: `godo diff`, `--base <commit>` override, `--pager <cmd>` to override
    git's pager config, and `--no-pager` to force raw output.
5. [x] Define exit codes for invalid sandbox, missing base, and git failures using existing
    conventions if present; otherwise introduce explicit codes.

2. Stage Two: Metadata + git helpers
1. [x] Store sandbox metadata under the godo project directory (path and filename to be selected),
    containing base commit/ref and created time; defer schema versioning for now.
2. [x] Record base commit at sandbox creation and preserve it on reuse; delete on remove/clean.
3. [x] Add git helpers for rev-parse verification and merge-base resolution.
4. [x] Add helper to list untracked files honoring standard ignore rules.
5. [x] Implement diff runner that emits:
     - `git diff <base>` for tracked changes (staged + unstaged + committed in sandbox).
     - per-file diffs for untracked files using `git diff --no-index /dev/null <file>`.

3. Stage Three: Godo API + CLI integration
1. [x] Add a `Godo::diff` entry point that validates sandbox liveness and resolves base commit.
2. [x] Execute diff using `--no-pager` or `-c core.pager=<cmd>` when requested.
3. [x] Keep diff content on stdout/stderr while using `Output` for status/errors only.
4. [x] Wire the new subcommand in `crates/godo/src/main.rs` and propagate exit codes.

4. Stage Four: Tests, docs, validation
1. [x] Add unit tests for metadata read/write and base resolution edge cases.
2. [x] Add integration tests for staged, unstaged, committed, and untracked diffs.
3. [x] Add integration test for `--base` override.
4. [x] Update README with usage examples and pager configuration (delta).
5. [x] Run clippy, tests, and fmt per project guidance.
