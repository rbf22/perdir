# Contributing

This project is intentionally small at first.

Good first issues:

- Improve manifest validation
- Add shell completion
- Add `perdir doctor`
- Add Bubblewrap detection
- Add Nix flake generation
- Add tests for manifest parsing

Principles:

1. Directory environments should be inspectable.
2. AI should propose changes as diffs, not silently mutate the host.
3. Linux primitives should be reused before inventing new ones.
4. Isolation should become the default over time.
