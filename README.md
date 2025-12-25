![Discord](https://img.shields.io/discord/1381424110831145070?style=flat-square&logo=rust&link=https%3A%2F%2Fdiscord.gg%2FfHmRmuBDxF)
[![Crates.io](https://img.shields.io/crates/v/godo)](https://crates.io/crates/godo)


# godo

Fast, parallel sandboxes for any Git project.

`godo` creates copy-on-write worktrees so you can run tests, generators or
one-off tools in isolation. Each sandbox is disposable, disk-cheap and mapped
to its own branch, letting you merge the results whenever you are ready.

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

Run `cargo fmt` in an isolated workspace that is cleaned up afterwards:

```bash
$ godo run format cargo fmt
```

1. A worktree appears in `~/.godo/<project>/format`.
2. `cargo fmt` runs there, touching only CoW copies.
3. The tool prompts you for a commit message and commits the result to branch `godo/format`.
4. The sandbox is deleted (pass `--keep` to keep it).

---

## Features at a glance

* Works from any Git repository.
* Sandboxes live under `~/.godo/<project>/<name>` (configurable with `--dir` or `GODO_DIR`).
* The full file tree – except `.git/` – is cloned using copy-on-write where the
  filesystem supports it (APFS, Btrfs, ZFS…).
* Exclude paths with repeated `--exclude <glob>` flags.
* Each sandbox is backed by branch `godo/<name>`.
* Automatic cleanup; keep a sandbox with `--keep` or auto-commit with
  `--commit "msg"`.
* Exit codes from commands are preserved, making `godo` scriptable.
* Direct exec preserves quoting and args; use `--sh` for shell features
  like pipes, globs, and redirection.

---

## Command guide

```bash
Usage: godo [OPTIONS] <COMMAND>

Commands:
  run     Run a command in an isolated workspace
  diff    Diff a sandbox against its base commit
  list    Show existing sandboxes
  remove  Delete a named sandbox
  clean   Clean up a sandbox; removes unmodified worktree and fully merged branch
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

### Diffing a sandbox

Show tracked and untracked changes in a sandbox compared to its recorded base commit:

```bash
godo diff my-sandbox
```

Override the base commit or pager configuration:

```bash
godo diff --base HEAD~1 my-sandbox
godo diff --pager "delta" my-sandbox
godo diff --no-pager my-sandbox
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
   Uses [clonetree](https://github.com/cortesi/clonetree) to copy the working
   tree, skipping `.git/` and honouring `--exclude <glob>` rules. On
   copy-on-write filesystems this is instantaneous and consumes no
   additional space.

4. **Run the command or shell**  
   By default, execs the program directly with its arguments (no extra shell),
   preserving argument boundaries and quoting: `prog arg1 "a b"` becomes
   `Command::new("prog").args(["arg1", "a b"])`. Pass `--sh` to force shell
   evaluation via `$SHELL -c "<COMMAND>"` (useful for pipes, globs, etc.). If no
   command is provided, an interactive shell is opened in the sandbox. The exit
   code from the command is preserved and passed back to the caller, allowing
   `godo` to be used in scripts and CI pipelines.

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

---

## Library crates

`godo` is built on two internal crates so other frontends can reuse the core:

- **libgodo**: the sandbox engine (worktrees, status, cleanup). It is UI-agnostic and performs no
  user I/O.
- **godo-term**: terminal rendering, prompts, and spinners used by the CLI.

If you are integrating godo into a GUI or other tool, depend on `libgodo` directly and supply
your own UI layer.

---

## Filesystem support

`godo` uses [clonetree](https://github.com/cortesi/clonetree) for efficient
file cloning. Performance depends on your filesystem's copy-on-write (CoW)
capabilities:

### Filesystems with native CoW support

- **macOS 10.13+** / APFS
- **iOS** / APFS
- **Linux 6.7+** / Btrfs
- **Linux 5.4+** / XFS (with `reflink=1`)
- **Linux 6.1+** / bcachefs
- **Linux 5.13+** / overlayfs
- **Windows Server 2016+** / ReFS

### Filesystems without CoW support

- **ext4** (Ubuntu/Fedora default) - Falls back to byte-for-byte copy

On CoW-enabled filesystems, cloning is near-instantaneous and uses no
additional disk space until files are modified. On other filesystems, a full
copy is made, which may take longer for large repositories.
