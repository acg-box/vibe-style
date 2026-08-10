# Files

- [vibe-style CLI](cli.md) - Command-line contract for curate, tune, and coverage, including cargo-compatible target selection, language requirements, exit behavior, and argument normalization.
- [Check and Fix Engine](engine.md) - File discovery, parallel checking, bounded tune rounds, edit application, state refresh, and convergence behavior implemented by src/style.rs and shared helpers.
- [vibe-style Architecture Overview](overview.md) - Runtime composition and ownership boundaries for the Rust vstyle and cargo-vstyle binaries, Rust and Swift style lanes, diagnostics, fixes, and semantic validation.
- [Style Rule Families and Change Surface](rules.md) - Registered stable rule IDs, family ownership, backend classification, fixability boundaries, interactions, and test navigation for Rust and Swift checks.
- [Semantic Validation and Cache](semantic-validation.md) - Cargo check integration, compiler-error diff validation, import suggestion rounds, semantic cache keys, rollback boundaries, and benchmark routing.
- [Swift Source-Text Backend](swift.md) - Conservative read-only Swift checks, masking rules, test-file exception, applicability boundary, and focused integration coverage.
- [Testing, CI, Release, and OpenWiki Automation](testing-and-automation.md) - Repository task entrypoints, focused and broad validation, composite Action behavior, release publication, benchmark workflow routing, and scheduled OpenWiki updates.
