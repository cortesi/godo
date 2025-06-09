# godo

**Instant parallel sandboxes for any Git project on macOS**

`godo` spins up isolated, copy‑on‑write workspaces (APFS `clonefile` + `git
worktree`) so you can run tests, generators, or one‑off tools in parallel
without touching your main working copy. When the task finishes, changes are
committed to a branch you can merge whenever you like.

---

## Quick start

```bash
# Run rustfmt in an isolated workspace that disappears when done
$ godo run format cargo fmt
```

1. A worktree appears in `~/.godo/format`.
2. `cargo fmt` runs there, touching only CoW copies.
3. The tool prompts you for a commit message and writes changes to `godo/format`.
4. The sandbox is deleted (pass `--persist` to keep it).

---

## Features at a glance

* Runs from inside **any** Git repo.
* Sandboxes live under `~/.godo/<name>` by default.
* Customize the godo directory with `--dir` flag or `GODO_DIR` environment variable.
* By default every untracked item-even those in `.gitignore` - is copied into the
  sandbox using APFS copy‑on‑write.  Limit what is copied with `--copy`.
* Results land on branch `godo/<name>`.
* The sandbox is automatically removed unless you keep it with `--persist`.

---

## Command guide

### Global options

* `--dir <DIR>` - Override the default godo directory location (`~/.godo`).
* `GODO_DIR` environment variable - Set a custom godo directory location.

Priority: `--dir` flag > `GODO_DIR` env var > default `~/.godo`

### `godo run`

```
godo run [--persist] [--copy <glob>]... <name> [COMMAND]
```

* `--persist` - keep the sandbox after the command exits.  Handy for manual
  inspection or re‑runs.
* `--copy <glob>` - copy only directories that match `glob` into the sandbox.
  You can specify this flag multiple times.  If you omit it, **all** untracked
  items are copied.
* If you omit `COMMAND`, `godo` drops you into an interactive shell inside the
  sandbox.

---

### `godo list`

Shows existing sandboxes that are still on disk (either running or created with `--persist`).

### `godo rm <name>`

Deletes the named sandbox directory and detaches its worktree.

### `godo prune`

Removes sandbox directories whose branch no longer exists, freeing disk space.

---

## Installation

```bash
cargo install godo
```

**Requirements**

* macOS 11 or newer (APFS)
* Git ≥ 2.30

---

## How it works

1. **Detect repository** - walks up from the current directory until a `.git`
   folder is found.
2. **Create worktree** - `git worktree add --quiet ~/.godo/<name> HEAD`
   shares the parent repository’s object database, so no blobs are duplicated.
3. **Copy resources** - for each pattern given with `--copy` (or for every
   untracked item if no patterns) `godo` runs `cp -cR <resource>
   ~/.godo/<name>/`.  APFS performs a `clonefile(2)` call, so the copy is
   instant and copy‑on‑write.
4. **Run command or shell** - executes `$SHELL -c "<COMMAND>"` inside the
   sandbox, or opens an interactive shell if no command was supplied.  All
   writes stay inside the sandbox.
5. **Commit results** - switches to `godo/<name>`, stages changes, prompts for
   a commit message, and writes the commit. The branch is already attached to
   the parent repository.
6. **Cleanup** - unless `--persist` was specified, `godo` runs `git worktree
   remove --force ~/.godo/<name>` and deletes the sandbox directory.

