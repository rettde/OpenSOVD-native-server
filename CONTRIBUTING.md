# Contributing to OpenSOVD-native-server

Thank you for your interest in contributing to the Eclipse OpenSOVD project!

## Eclipse Contributor Agreement (ECA)

Before your contribution can be accepted, you must complete the
[Eclipse Contributor Agreement (ECA)](https://www.eclipse.org/legal/ECA.php).

This is a one-time process for all Eclipse Foundation projects.

## How to Contribute

1. **Fork** the repository and create a feature branch from `main`.
2. **Implement** your changes, following the existing code style and patterns.
3. **Add or update tests** — all changes should maintain or improve test coverage.
4. **Run the quality gate** before submitting:
   ```bash
   make check     # clippy pedantic + full test suite
   ```
   Or manually:
   ```bash
   cargo clippy --workspace -- -D warnings
   cargo test --workspace
   ```
5. **Commit** with clear, descriptive messages following [Conventional Commits](https://www.conventionalcommits.org/).
6. **Open a Pull Request** against `main` with a description of your changes.

## Code Style

- Follow existing Rust idioms and patterns in the codebase.
- Workspace-level Clippy pedantic lints are enforced via `[workspace.lints]` in `Cargo.toml`.
- All source files must include the SPDX license header:
  ```rust
  // Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
  // SPDX-License-Identifier: Apache-2.0
  ```
- Use `#![forbid(unsafe_code)]` in all crates except `native-comm-someip` (vSomeIP FFI).
- Zero Clippy warnings required.

## Project Structure

This project is part of the [Eclipse OpenSOVD](https://github.com/eclipse-opensovd) ecosystem.
It implements the SOVD Server role from the
[OpenSOVD design](https://github.com/eclipse-opensovd/opensovd/blob/main/docs/design/design.md).

Related repositories:
- [opensovd-core](https://github.com/eclipse-opensovd/opensovd-core) — Server, Client, Gateway (C++)
- [classic-diagnostic-adapter](https://github.com/eclipse-opensovd/classic-diagnostic-adapter) — CDA (Rust)
- [COVESA/vsomeip](https://github.com/COVESA/vsomeip) — SOME/IP reference implementation

## Reporting Issues

Please use the GitHub issue tracker to report bugs or request features.
Include steps to reproduce, expected vs. actual behavior, and your environment.

## License

By contributing, you agree that your contributions will be licensed under
the [Apache License, Version 2.0](LICENSE).
