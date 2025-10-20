# Coverage Improvement Plan (2025-10-19)

## Current Snapshot
- Coverage gathered on 2025-10-19 via `cargo tarpaulin --out Json --output-dir target/tarpaulin`.
- Latest result after the expanded test suite: **100.00 %** line coverage (1270/1270 lines) for `src/main.rs`, with the refreshed trace stored at `target/tarpaulin/mdkloc-coverage.json`.
- All previously uncovered branches are now exercised; the additional tests guard comment/blank-line parsing across every supported language as well as the CLI entry points.

## Reinforcements Added

| Area | What changed |
| --- | --- |
| Metrics & entrypoint | Added verification around `PerformanceMetrics::new`, `run_with_args`, and `main` (via a test-only argument override) so the CLI paths execute in-process without spawning a subprocess. |
| C-style & JavaScript analyzers | Introduced fixtures covering blank lines, unterminated block comments, and mixed `//`/`/*`/`<!--` scenarios—including JSX comments with leading code. |
| Legacy language parsers | Expanded coverage for Pascal, IPLAN, PowerShell, and other classic formats to ensure trailing code after comment delimiters and blank-line bookkeeping are validated. |
| Language blank-line suite | Added a table-driven test that exercises blank-line handling for PHP, Ruby, Shell, ASM, DCL, Batch, TCL, and XML. |
| Filesystem edge cases | Added tests for invalid UTF-8 paths and special filesystem entries (e.g., Unix sockets) so `filespec_matches` and `scan_directory_impl` hit their error-avoidance branches. |

## Follow-Up
- Keep `cargo tarpaulin --out Json --output-dir target/tarpaulin` in the workflow to detect regressions; the stored JSON makes diffing future runs straightforward.
- Consider enforcing a CI coverage threshold (≥100 % now achievable) to lock in the gains and surface regressions early.
