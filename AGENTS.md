# AGENTS.md

This file documents the use of AI coding agents in this project.

## AI-Assisted Development

This project was developed with significant assistance from **Windsurf Cascade**
(Anthropic Claude), an AI pair-programming agent integrated into the developer's IDE.

### Scope of AI contribution

| Area | AI involvement |
|------|---------------|
| **Architecture design** | Co-designed: gateway vs. standalone mode, `ComponentBackend` trait, `ComponentRouter` pattern |
| **Rust implementation** | All workspace crates (`native-*`) were authored by the AI agent based on human requirements and iterative review |
| **ISO 17978-3 conformance** | AI implemented all 51 mandatory SOVD requirements; human verified against standard |
| **Test suite** | 227 unit/integration tests authored by AI, validated by human |
| **Documentation** | README, CONTRIBUTING, NOTICE, CHANGELOG, architecture docs — AI-drafted, human-reviewed |
| **CI/CD** | GitHub Actions workflow authored by AI |

### Human oversight

All code was reviewed and approved by the project maintainer before commit.
Architectural decisions (ecosystem alignment with Eclipse OpenSOVD, ISO 17978-3
instead of ASAM SOVD reference, COVESA/vsomeip integration strategy) were made
by the human developer.

### Tools used

- **Windsurf Cascade** (Claude) — Primary coding agent
- **Cargo / Clippy / rustfmt** — Automated quality gates
- Human review on every change

### Why disclose this?

Transparency about AI involvement helps reviewers and contributors understand
the project's development history. It does not diminish the quality of the code —
all output was verified through tests, linting, and manual review.
