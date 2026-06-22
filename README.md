# perdir

Per-directory Linux environments.

`perdir` is an experimental CLI for treating every project directory as its own declared runtime, policy boundary, and AI context.

## MVP

```bash
perdir init
perdir status
perdir run env
perdir enter
perdir explain
```

This first version does **not** replace containers, Nix, or the Linux kernel. It creates the manifest and command surface that later versions can back with Nix, Bubblewrap, cgroups, seccomp, and AI-assisted patch generation.

## Manifest

Each project gets:

```text
.perdir/
  world.toml
  memory.md
  audit.log
```

Example:

```toml
name = "example"

[runtime]
python = "3.12"
packages = ["python"]

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
- [ ] Bubblewrap-backed filesystem isolation
- [ ] Permission prompts and policy enforcement
- [ ] AI command: propose manifest changes as reviewable diffs
- [ ] Rollbackable environment transactions
- [ ] Shell integration for automatic directory activation
