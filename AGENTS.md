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
- **cache.rs** - Cache state management, stored as JSON in `.verify/cache.json`
- **hasher.rs** - BLAKE3 file hashing for change detection
- **runner.rs** - Check execution with dependency ordering and parallel execution
- **graph.rs** - Dependency graph using petgraph, topological sorting, parallel "wave" grouping
- **ui.rs** - Terminal output with colors and progress indicators
- **output.rs** - JSON output formatting for tool integration
- **metadata.rs** - Regex-based metric extraction from command output

### Key Flows

**Staleness Detection**: A check is stale if:
1. Files matching `cache_paths` changed since last successful run
2. Any dependency (verification or subproject) is stale
3. Last run failed
4. No `cache_paths` defined (always runs)

**Execution Model**: Checks are grouped into "waves" - independent checks within a wave run in parallel via rayon, waves execute sequentially to respect dependencies.

**Exit Codes**: 0 (success), 1 (failures), 2 (configuration error)

## Configuration Format (verify.yaml)

```yaml
verifications:
  - name: check_name
    command: npm run build
    cache_paths:
      - "src/**/*.ts"
    depends_on: [other_check]  # optional
    timeout_secs: 300          # optional
    metadata:                   # optional - regex extraction
      key: "pattern"

  - name: frontend
    path: packages/frontend  # references another verify.yaml
```

## Test Fixtures

The `test-project/` directory contains example configurations with subprojects demonstrating:
- Root config with subproject references
- Cross-project dependencies
- Nested cache directories
