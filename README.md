# vfy

A fast, lightweight CLI for managing project verification checks with intelligent caching.

## Why vfy?

In agent-driven development, verification is repetitive. You run the same checks over and over: typecheck, lint, test, build. Existing tools like Nx and Turborepo are powerful but heavyweight and ecosystem-specific.

**vfy** is different:
- **Standalone** - Works with any project, any language
- **Simple** - One YAML file, one binary
- **Fast** - BLAKE3 hashing, parallel execution
- **Smart** - Only re-runs checks when relevant files change

## Installation

```bash
cargo install --path .
```

Or build from source:

```bash
cargo build --release
# Binary at ./target/release/vfy
```

## Quick Start

```bash
# Create a config file
vfy init

# Edit vfy.yaml to define your checks
# Then run:
vfy status  # See what needs to run
vfy         # Run stale checks
```

## Configuration

Create a `vfy.yaml` in your project root:

```yaml
verifications:
  - name: build
    command: npm run build
    cache_paths:
      - "src/**/*.ts"
      - "package.json"

  - name: typecheck
    command: npm run typecheck
    cache_paths:
      - "src/**/*.ts"
      - "tsconfig.json"

  - name: test
    command: npm test
    depends_on: [build]
    cache_paths:
      - "src/**/*.ts"
      - "tests/**/*.ts"

  - name: lint
    command: npm run lint
    cache_paths:
      - "src/**/*.ts"
      - ".eslintrc*"
```

### Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique identifier for the check |
| `command` | Yes | Shell command to execute |
| `cache_paths` | Yes | Glob patterns for files that affect this check |
| `depends_on` | No | List of checks that must pass first |
| `timeout_secs` | No | Command timeout (not yet implemented) |

## Usage

### Check Status

```bash
vfy status
```

Output:
```
✓ build - fresh (ran 2m ago, 3.1s)
✓ typecheck - fresh (ran 2m ago, 2.0s)
○ test - stale (depends on: build)
○ lint - stale (3 files changed)
? e2e - never run
```

### Run Checks

```bash
vfy              # Run all stale checks
vfy run build    # Run specific check (and dependencies)
vfy run --all    # Force run all checks
vfy run --force  # Force run even if fresh
```

### JSON Output

For tool integration:

```bash
vfy --json status
vfy --json run
```

Example output:
```json
{
  "checks": [
    {
      "name": "build",
      "status": "fresh",
      "last_run": "2024-01-15T10:30:00Z",
      "duration_ms": 3200
    },
    {
      "name": "test",
      "status": "stale",
      "reason": "dependency_stale",
      "stale_dependency": "build"
    }
  ]
}
```

### Clear Cache

```bash
vfy clean           # Clear all cached results
vfy clean build     # Clear specific check
```

## How It Works

1. **File Hashing**: vfy computes BLAKE3 hashes of all files matching `cache_paths`
2. **Cache Storage**: Results are stored in `.vfy/cache.json`
3. **Staleness Detection**: A check is stale if:
   - Files in `cache_paths` changed since last successful run
   - Any dependency is stale
   - Last run failed
4. **Parallel Execution**: Independent checks run concurrently
5. **Dependency Ordering**: Checks run in topological order respecting `depends_on`

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | All checks passed (or skipped as fresh) |
| 1 | One or more checks failed |
| 2 | Configuration error |

## Integration Ideas

- **Git hooks**: Run `vfy` in pre-commit or pre-push
- **CI/CD**: Use `vfy --json` for structured output
- **Agent tools**: Parse JSON to show verification status in UIs
- **Watch mode**: Combine with `watchexec` or similar

## License

MIT
