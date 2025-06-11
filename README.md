![Discord](https://img.shields.io/discord/1381424110831145070?style=flat-square&logo=rust&link=https%3A%2F%2Fdiscord.gg%2FfHmRmuBDxF)
[![Crates.io](https://img.shields.io/crates/v/godo)](https://crates.io/crates/godo)
[![docs.rs](https://img.shields.io/docsrs/godo)](https://docs.rs/godo)


# godo

**Fast parallel sandboxes for any Git project**

`godo` spins up isolated, copy‑on‑write workspaces (`clonetree` + `git
worktree`) so you can run tests, AI code generators, or one‑off tools in
parallel without touching your main working copy. When the task finishes,
changes are committed to a branch you can merge whenever you like.

---

## Community

Want to contribute? Have ideas or feature requests? Come tell us about it on
[Discord](https://discord.gg/fHmRmuBDxF). 


---

## Installation

```bash
cargo install godo
```

---

## Quick start

Run `cargo fmt` in an isolated workspace that disappears when done:

```bash
$ godo run format cargo fmt
```

1. A worktree appears in `~/.godo/format`.
2. `cargo fmt` runs there, touching only CoW copies.
3. The tool prompts you for a commit message and writes changes to `godo/format`.
4. The sandbox is deleted (pass `--keep` to keep it).

---

## Features at a glance

* Runs from inside **any** Git repo.
* Sandboxes live under `~/.godo/<name>` by default.
* Customize the godo directory with `--dir` flag or `GODO_DIR` environment variable.
* By default every untracked item-even those in `.gitignore` - is copied into the
  sandbox using APFS copy‑on‑write.  Limit what is copied with `--copy`.
* Results land on branch `godo/<name>`.
* The sandbox is automatically removed unless you keep it with `--keep`.

---

## Command guide

```bash
Usage: godo [OPTIONS] <COMMAND>

Commands:
  run     Run a command in an isolated workspace
  list    Show existing sandboxes
  remove  Delete a named sandbox
  help    Print this message or the help of the given subcommand(s)

Options:
      --dir <DIR>       Override the godo directory location
      --repo-dir <DIR>  Override the repository directory (defaults to current git project)
      --color           Enable colored output
      --no-color        Disable colored output
      --quiet           Suppress all output
      --no-prompt       Skip confirmation prompts
  -h, --help            Print help
  -V, --version         Print version
```

---

## How it works

1. **Detect repository**  
   From the current directory (or `--repo-dir` if specified), walks up until a
   `.git` folder is found to identify the parent Git repository.

2. **Prepare sandbox workspace**  
   Ensures the root godo directory (default `~/.godo`, configurable via `--dir`
   or `GODO_DIR`) and per-project subdirectory exist, then runs:

   ```bash
   git worktree add --quiet -b godo/<name> ~/.godo/<project>/<name>
   ```

   to create a new worktree on branch `godo/<name>` at `HEAD` without duplicating
   objects.

3. **Clone the file tree**  
   Uses the [clonetree](https://github.com/cortesi/clonetree) crate to clone
   the repository tree into the sandbox, skipping `.git/` and applying any
   `--exclude <glob>` patterns. On CoW-enabled filesystems (e.g. APFS) this
   leverages `clonefile(2)` for instant copy-on-write clones.

4. **Run the command or shell**  
   Invokes `$SHELL -c "<COMMAND>"` (or drops into an interactive shell if no
   command is provided) inside the sandbox, so all writes and changes remain
   isolated.

5. **Commit or keep results**  
   Unless `--keep` is specified, godo prompts to:

   - **commit:** stage all changes (`git add .`) and run `git commit --verbose`
     on branch `godo/<name>`.
   - **shell:** open an interactive shell in the sandbox (then clean up on exit).
   - **keep:** leave the sandbox intact for manual inspection or re-runs.

6. **Cleanup**  
   After committing (or after the optional shell stage), godo automatically
   removes the worktree (`git worktree remove`) and deletes the sandbox directory.
   If the `godo/<name>` branch has no unmerged commits, the branch is also
   deleted.

7. You can now merge the changes from `godo/<name>` into your main branch
   whenever you like.
