# perdir

Per-directory Linux environments.

`perdir` is an experimental CLI for treating every project directory as its own declared runtime, policy boundary, and AI context.

## Install

### From source

```bash
git clone https://github.com/YOUR_GITHUB_USERNAME/perdir.git
cd perdir
cargo install --path .
```

### Prerequisites

- [Rust](https://rustup.rs) (stable toolchain, includes `cargo`)

### Verify

```bash
perdir --help
```

## Getting Started

Create a new project directory and initialize a perdir environment:

```bash
mkdir my-project && cd my-project
perdir init
```

This creates a `.perdir/` folder containing `world.toml` (the manifest), `memory.md`, and an `audit.log`.

Check the current environment:

```bash
perdir status
```

Get a human-readable summary of the manifest:

```bash
perdir explain
```

Run a command inside the declared environment. Env vars from `world.toml` are applied, and if a Python version is declared, a venv is automatically created and activated:

```bash
perdir run python --version
```

The venv is stored at `.perdir/venv/` and packages from the manifest are installed automatically (only when the package list changes). Permission policies are checked before running:

- **`network = "ask"`** — prompts with `[y/N]` before running the command. Answering `n` aborts.
- **`network = "deny"`** — **enforced**. Uses OS-level sandboxing to block all network sockets in the child process: macOS Seatbelt (`kSBXProfileNoNetwork`) via `sandbox_init`, Linux network namespaces (`CLONE_NEWNET`) via `unshare`. The sandbox is applied in `pre_exec` after fork but before exec, so only the child process is restricted.
- **`home = "deny"` / `home = "read-only"`** — prints a warning but is **not enforced**. Filesystem sandboxing is on the roadmap.
- **`gpu = false`** — prints a notice but is **not enforced**.

Print shell exports for the environment. Running `perdir enter` alone shows what would be set:

```bash
$ perdir enter
export PERDIR_ROOT='/Users/you/my-project'
export PERDIR_NAME='my-project'
# Run this to enter:
# eval "$(perdir enter)"
```

To actually apply those exports to your current shell, use `eval`:

```bash
$ eval "$(perdir enter)"
$ echo "$PERDIR_NAME"
my-project
```

`perdir enter` prints `export` statements. `eval` runs them in your shell, setting `PERDIR_ROOT`, `PERDIR_NAME`, and any env vars from `world.toml`. This is useful when you want to run several commands in the environment without prefixing each one with `perdir run`.

View the audit log of all actions taken in this environment:

```bash
$ perdir log
2026-06-22T13:36:11+00:00 init
2026-06-22T13:43:00+00:00 run ["env"]
2026-06-22T13:43:55+00:00 run ["df"]
```

Edit the manifest directly in your `$EDITOR`:

```bash
perdir edit
```

Remove the venv to force a fresh rebuild on next `perdir run`:

```bash
perdir clean
```

Create the venv and install packages from the manifest:

```bash
perdir install
```

This creates `.perdir/venv/`, installs `pip_packages`, and writes `.perdir/perdir.lock` with exact installed versions. `perdir run` will use the existing venv without reinstalling.

Ask AI to propose manifest changes based on your context files and a prompt:

```bash
perdir ai "add numpy and pandas to pip_packages"
```

This reads your `ai.context` files and `memory.md`, sends them with the current manifest to an OpenAI-compatible API, and prints a proposed manifest. Answer `y` to apply it. Requires `OPENAI_API_KEY` (or set `OPENAI_BASE_URL` for a local model via Ollama/LM Studio). The model is determined by `ai.model` in the manifest (defaults to `gpt-4o`).

Check the manifest for issues (missing paths, empty fields, invalid values):

```bash
$ perdir validate
WARN: ai context path 'README.md' does not exist
WARN: ai context path 'src/' does not exist
```

## Shell Integration

Auto-activate the environment when you `cd` into a perdir directory. Add this to your `~/.zshrc` or `~/.bashrc`:

```bash
eval "$(perdir shell-init)"
```

Now whenever you enter a directory with a `.perdir/world.toml`, the environment variables and venv path are automatically applied. When you leave, they're unset.

## MVP

This first version does **not** replace containers, Nix, or the Linux kernel. It creates the manifest and command surface that later versions can back with Nix, Bubblewrap, cgroups, seccomp, and AI-assisted patch generation.

## Manifest

Each project gets:

```text
.perdir/
  world.toml
  memory.md
  audit.log
  venv/          # auto-created by perdir install, gitignore this
  perdir.lock    # pinned package versions, commit this
```

Add `.perdir/venv/` to your project's `.gitignore` (but commit `.perdir/perdir.lock` for reproducibility):

```gitignore
.perdir/venv/
```

Example:

```toml
name = "example"

[runtime]
python = "3.12"
packages = ["python"]
pip_packages = ["requests", "rich"]

[runtime.env]
RUST_LOG = "info"

[permissions]
network = "ask"
home = "read-only"
gpu = false

[ai]
context = ["README.md", "src/"]
memory_file = ".perdir/memory.md"
model = "local-or-cloud"
```

## Roadmap

- [ ] Nix-backed dependency resolution
- [ ] Filesystem isolation (home deny/read-only enforcement)
- [x] Network isolation (deny enforced via macOS Seatbelt / Linux namespaces)
- [x] Permission prompts and policy enforcement
- [ ] AI command: propose manifest changes as reviewable diffs
- [x] AI command: propose manifest changes via OpenAI-compatible API
- [ ] Rollbackable environment transactions
- [x] Shell integration for automatic directory activation
- [x] Venv management with lock file (perdir install / perdir clean)
