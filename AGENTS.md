# AGENTS.md

This file provides guidance to AI coding agents when working with code in this repository.

## Project Overview

**verify** is a Rust CLI tool for managing project verification checks (typecheck, lint, test, build) with intelligent caching. It uses BLAKE3 hashing to detect file changes and only re-runs checks when relevant files are modified.

## Build Commands

```bash
# Build debug
cargo build

# Build release
cargo build --release

# Install locally
cargo install --path .

# Run against test project
./test-project.sh [args]  # Builds and runs: ./target/debug/verify --config test-project/verify.yaml run [args]
```

## Architecture

The codebase is organized into focused modules in `src/`:

- **main.rs / cli.rs** - Entry point and CLI parsing (subcommands: `init`, `status`, `run`, `clean`)
- **config.rs** - YAML configuration parsing and validation (checks for cycles, duplicates, unknown deps)
- **cache.rs** - Cache state management, stored as JSON in `verify.lock` (committable lock file at project root)
- **hasher.rs** - BLAKE3 file hashing for change detection
- **runner.rs** - Check execution with dependency ordering and parallel execution
- **graph.rs** - Dependency graph using petgraph, topological sorting, parallel "wave" grouping
- **ui.rs** - Terminal output with colors and progress indicators
- **output.rs** - JSON output formatting for tool integration
- **metadata.rs** - Regex-based metric extraction from command output

### Key Flows

**Verification Status** (`VerificationStatus` enum in cache.rs):
- `Verified` - Check passed and files haven't changed
- `Unverified { reason }` - Check needs to run
- `Untracked` - Check has no `cache_paths`, so changes can't be tracked (always runs)

A check is **unverified** if:
1. Files matching `cache_paths` changed since last successful run
2. Check definition changed in verify.yaml (detected via `config_hash` - includes command, cache_paths, timeout, per_file, metadata patterns)
3. Any dependency (verification or subproject) is unverified
4. Last run failed or never run

**Unverified Reasons** (`UnverifiedReason` enum in cache.rs):
- `FilesChanged` - Files in cache_paths have changed
- `ConfigChanged` - The check definition changed in verify.yaml
- `DependencyUnverified` - A dependency is unverified
- `NeverRun` - Never run or no successful run recorded

**Aggregate Checks**: Checks can omit the `command` field to create aggregate checks whose status is derived purely from their dependencies. Useful for grouping related checks.

**Execution Model**: Checks are grouped into "waves" - independent checks within a wave run in parallel via rayon, waves execute sequentially to respect dependencies.

**Per-File Mode**: When `per_file: true`, the command runs once per stale file with `VERIFY_FILE` env var. Progress is preserved even when the overall check fails:
- Files that passed are tracked individually in `file_hashes`
- On re-run, only files that failed or changed since passing are re-executed
- Cache is saved after each file passes (interrupt-safe)

### Cache Format (verify.lock)

The cache is stored as `verify.lock` in each project/subproject root. Designed to:
- Share verification state between local development and CI
- Travel with git branches and worktrees
- Have minimal diffs (no timestamps or durations)

**Structure:**
```json
{
  "version": 4,
  "checks": {
    "check_name": {
      "config_hash": "...",      // Hash of check definition
      "content_hash": "...",     // Hash of all files (null if last run failed)
      "file_hashes": {},         // Only for per_file checks
      "metadata": {}             // Extracted metrics
    }
  }
}
```

On `verify init`, `.gitattributes` is updated with `verify.lock merge=ours` for merge conflict handling.

**Exit Codes**: 0 (success), 1 (failures), 2 (configuration error)

## Configuration Format (verify.yaml)

```yaml
verifications:
  - name: check_name
    command: npm run build       # optional - omit for aggregate checks
    cache_paths:
      - "src/**/*.ts"
    depends_on: [other_check]  # optional
    timeout_secs: 300          # optional
    per_file: false            # optional - run once per stale file with VERIFY_FILE env var
    metadata:                   # optional - regex extraction
      key: "pattern"

  - name: all                  # aggregate check - status derived from dependencies
    depends_on: [check_name, frontend]

  - name: frontend
    path: packages/frontend  # references another verify.yaml
```

## Test Fixtures

The `test-project/` directory contains example configurations with subprojects demonstrating:
- Root config with subproject references
- Cross-project dependencies
- Per-subproject `verify.lock` files
- Metadata extraction examples
- Per-file mode usage

## Releasing

See [RELEASE.md](RELEASE.md) for instructions on creating releases.
