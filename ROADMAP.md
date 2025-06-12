
## 1. Short-term wins (0-3 releases)

These items are scoped so that each can be finished in <1 day of work and shipped in the next point release.  They are ordered by expected impact on reliability first, then user experience.

| Area | Task | Notes |
|------|------|-------|
|Correctness|✔️ Atomic work-tree creation|Use `git worktree add --lock`/temporary dir + rename to avoid half-created sandboxes if the process is interrupted.|
|Correctness|✔️ Handle existing remote branches|`git::has_branch` currently only checks local branches.  Extend to `--remotes` to avoid accidental name clashes.|
|Correctness|✔️ Exit codes|`godo run …` should return the underlying command’s exit code so that it can be scripted.|
|Correctness|✔️ Windows CI|The code compiles on Windows but many tests that rely on reflinks/APFS assumptions fail.  Introduce conditional cloning strategy and run GitHub Actions on Windows.| 
|Correctness|✔️ Graceful SIGINT|Pressing Ctrl-C inside the managed command can leave the work-tree locked.  Add signal handler that cleans up before exiting.|
|UX|✔️ Progress indicators|Long `clonetree` operations can look hung on slow disks.  Emit simple dots / spinners.|
|UX|✔️ `godo list --json`|Machine-readable output will allow editor integrations to surface sandbox status.|
|UX|✔️ Autocompletion scripts|Generate zsh/bash completion using clap facilities.|
|Packaging|✔️ `cargo install --locked` path|Update README and CI to use `--locked` for reproducible builds.|
|Docs|✔️ Man-page generation|`cargo xtask dist` to create `godo.1` from clap help and ship in releases.| 

---

## 2. Mid-term roadmap (≈3-6 months)

These features require several PRs each, but can be delivered incrementally.

1. Sandbox lifecycle API   
   – Expose `godo daemon` with a JSON-RPC / UNIX-socket interface.   
   – Allow editors/IDEs to request, reuse and dispose sandboxes without invoking processes.

2. Workspace caching   
   – Detect identical `clone_tree` inputs (repo SHA + exclude set) and hard-link to an existing sandbox to make creation O(1).

3. Pluggable storage back-ends   
   – Local reflinks (today)   
   – OverlayFS (Linux root-less)   
   – Copy-only fallback (FAT/NTFS)   
   – Remote volume over sshfs (stretch goal).

4. Config file (`godo.toml`)   
   – Per-repo defaults for excludes, color, default sandbox names.   
   – Global config at `~/.config/godo/config.toml`.

5. Interactive TUI (`godo ui`)   
   – Browse sandboxes, inspect diffs, drop to shell, commit, merge.   
   – Implemented with crossterm+ratatui.

6. “Smart” commit flow   
   – Detect formatted files/updated tests and propose conventional commit messages.   
   – Optionally push branch to remote and open a PR (GitHub/GitLab API).

7. Authentication/context sharing   
   – Copy `.git/config` remotes/pat configuration or respect `includeIf` rules so that pushes inside sandboxes just work.

---

## 3. Long-term vision (>6 months)

* Cloud sandboxes (ephemeral VMs or containers).  `godo cloud run …` spins up short-lived builders on AWS/GCP.
* Language-aware tasks:   
  – Rust: auto-detect `cargo` sub-commands and optionally reuse target dir.   
  – Node/PNPM: mount `node_modules` as read-only cache across sandboxes.  
* Merge-queue integration to pre-build CI artifacts for every branch queued for review.
* Plugin SDK so that custom corporate workflows (e.g. code generation, schema migrations) can hook into sandbox creation/cleanup.

---

## 4. Engineering quality goals

1. 100 % unit-test and ≥85 % integration-test coverage for the core rust crate.
2. All commands benchmarked on a large monorepo; <250 ms interactive latency for `godo list`.
3. `clippy --deny warnings` and `cargo audit` clean on every commit.
4. Stable, documented public Rust API so **godo** can be embedded in other tools.

---

## 5. Release cadence

* Patch releases (x.y.z) every time a short-term fix lands.  
* Minor releases (x.y.0) at the end of each mid-term milestone, ~every 6-8 weeks.  
* Major version **1.0** once the daemon API and remote backend abstraction are proven stable.

---

## 6. Contributing

See `CONTRIBUTING.md` (to be created) for coding standards, review expectations and how to run the full test-suite locally.  Join us on Discord to pick an item!  

---

*Happy sandboxing!* 🚀



## 0–8 WEEKS | Short-term wins (UX, correctness, “small but sweet”)

CLI polish & discoverability
• Ship shell completions for bash / zsh / fish (clap_complete).
• Add godo help <topic> pages and man-page generation.
• Support godo open <name> → spawns a shell or $EDITOR in an existing sandbox.
• Colour-coded, column-aligned godo list with --json for scripts.

Better prompts & defaults
• Remember the last commit message and pre-fill it.
• --default <action> flag that auto-selects commit/keep/shell when running non-interactively (handy for CI).

Safety / correctness
• Harden path handling against accidental traversal (prefer path.absolutize() + canonicalize).
• Switch from string-shelling out of git to the git2 crate where trivial (branch existence, status checks) to avoid parsing issues.
• Detect unsaved work in the calling repo and bail unless --force. The check already exists; surface it sooner in the run flow.

Quality of life utilities
• godo repeat <name> – rerun the last command that was executed in the sandbox.
• --base <ref> when creating a worktree so you can branch off anything, not only HEAD.

CI / release hygiene
• Add cargo deny + cargo audit and run in GitHub Actions.
• Publish pre-built binaries via cargo-dist so users don’t need a Rust tool-chain.

Documentation
• Add architecture overview diagram to README.
• Record a 90-second “getting started” asciinema / gif.

-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------

## 2–6 MONTHS | Medium-term feature work & platform reach

Smart sync / merge flows
• godo merge <name> – fast-forward main → sandbox, or rebase sandbox → main, with conflict preview.
• Optional “autosync” that periodically pulls main into every live sandbox.

Workflows & automation
• godo watch -- <cmd> – file-watcher that reruns a command in the sandbox on every change (think test loops).
• Template system (~/.config/godo/templates/*.toml) to pre-install tools or auxiliary files into new sandboxes.

Cross-platform support
• Official Windows support using NTFS ReFS block-clone or fallback to hard-links when CoW is unavailable.
• WSL detection: automatically create the sandbox inside the distro when invoked from Windows.

Extensibility / API
• Expose a small Rust library crate (godo-core) that wraps the Git + clonetree logic.
• Stable JSON output for all commands → makes writing wrappers / IDE integrations trivial.

Project health
• >80 % unit-test coverage for git.rs and godo.rs, plus smoke-tests that spin up temporary repos.
• Add dependabot + MSRV policy.

-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------

## 6–18 MONTHS | Long-term vision

Orchestration & parallelism
• “Matrix” mode: godo run --matrix pkg/* 'cargo test' spins one sandbox per glob and executes in parallel, merging results into dedicated branches.
• Remote executors: run a sandbox inside a container on another machine (ssh or Nomad) but keep the local branch workflow identical.

Pluggable back-ends
• File-system abstraction that supports Btrfs/CoW, ZFS clones, rsync --link-dest, and container layer copying so Godo stays fast everywhere.

Deep IDE / tool integrations
• VS Code extension: “Create sandbox”, “Open sandbox”, “Diff vs base” palette commands.
• GitHub Actions step that provisions a Godo sandbox, runs the job, and pushes the resulting branch.

Advanced merge assistance
• Automatic conflict grouping & TUI conflict resolver when merging many sandboxes back to main.
• Semantic “squash plan” that proposes sensible commit grouping based on per-sandbox history.

Security & isolation
• Optional user-namespace / seccomp hardening when running untrusted commands.
• Signed provenance file (godo.json) embedded in the commit trailer with the full command + toolchain versions.

Community & ecosystem
• “Awesome godo” repo listing templates and third-party helpers.
• Quarterly roadmap RFCs on Discord to keep direction transparent.

-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------

Milestone view
• v0.6 – “CLI delight”: completions, open, coloured list, safer git handling.
• v0.8 – “Workflow”: merge, watch, templates, Windows.
• v1.0 – “Platform”: remote sandboxes, pluggable back-ends, public API, high test coverage.
• v1.2 – “Ecosystem”: IDE plugin, Actions, advanced merge support.

-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------

Why this ordering?

    1. Quick UX and safety improvements make Godo more pleasant and trustworthy today.
    2. Mid-term items enlarge the set of everyday workflows you can replace with sandboxes.
    3. Long-term work turns Godo from a CLI utility into a full-blown workspace orchestration platform, opening doors to larger user bases and integrations.

Feel free to rearrange based on contributor bandwidth, but this sequence maximises visible user value while steadily paying down technical risk.
