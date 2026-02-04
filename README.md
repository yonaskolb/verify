# verify

A fast, lightweight CLI for managing project verification checks with intelligent caching.

## Why verify?

In agent-driven development, verification is key to assuring what gets built is correct. You want it to be fast and accurate. Instead of guiding the agent to run the same custom checks over and over again, and wasting tokens, you can run a single verify command that either gives the green light or provides the error context required to fix the problem.

- **Standalone** - Works with any project, any language
- **Simple** - One YAML file, one binary
- **Fast** - Written in rust, BLAKE3 hashing, parallel execution
- **Smart** - Only re-runs checks when relevant files change
- **Succinct** - Only returns the errors from your verifications, not the whole build output of every step
- **Open** - Use the json output to integrate into other tools like ui.

## Installation

```bash
cargo install --path .
```

Or build from source:

```bash
cargo build --release
# Binary at ./target/release/verify
```

## Quick Start

```bash
# Create a config file
verify init

# Edit verify.yaml to define your checks
# Then run:
verify status  # See what needs to run
verify         # Run stale checks
```

## Configuration

Create a `verify.yaml` in your project root:

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
| `cache_paths` | No | Glob patterns for files that affect this check. If omitted, check always runs |
| `depends_on` | No | List of checks or subprojects that must pass first |
| `metadata` | No | Regex patterns for extracting metrics from output |

### Subprojects

Reference other `verify.yaml` files in subdirectories:

```yaml
verifications:
  - name: frontend
    path: ./packages/frontend

  - name: backend
    path: ./packages/backend

  - name: integration
    command: npm run integration
    depends_on: [frontend, backend]  # Can depend on subprojects
    cache_paths:
      - "tests/**/*.ts"
```

Subprojects run their own verifications and can be dependencies for other checks.

### Metadata Extraction

Extract metrics from command output using regex patterns:

```yaml
verifications:
  - name: test
    command: npm test
    cache_paths:
      - "src/**/*.ts"
    metadata:
      passed: "Tests: (\\d+) passed"
      failed: "(\\d+) failed"
      coverage: "Coverage: ([\\d.]+)%"
```

Captured values are stored in the cache and displayed in status output. Supports:
- Simple patterns: Extract first capture group
- Replacement patterns: `["(\\d+)/(\\d+)", "$1 of $2"]` for formatted output

## Usage

### Check Status

```bash
verify status
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
verify                    # Run all stale checks
verify run build          # Run specific check (and dependencies)
verify run --force        # Force run even if fresh
verify run --verbose      # Stream command output in real-time
```

### JSON Output

For tool integration:

```bash
verify --json status
verify --json run
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
verify clean           # Clear all cached results
verify clean build     # Clear specific check
```

## How It Works

1. **File Hashing**: verify computes BLAKE3 hashes of all files matching `cache_paths`
2. **Cache Storage**: Results are stored in `.verify/cache.json`
3. **Staleness Detection**: A check is stale if:
   - Files in `cache_paths` changed since last successful run
   - Any dependency (check or subproject) is stale
   - Last run failed
   - No `cache_paths` defined (always runs)
4. **Parallel Execution**: Independent checks run concurrently
5. **Dependency Ordering**: Checks run in topological order respecting `depends_on`

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | All checks passed (or skipped as fresh) |
| 1 | One or more checks failed |
| 2 | Configuration error |

## Integration Ideas

- **Git hooks**: Run `verify` in pre-commit or pre-push
- **CI/CD**: Use `verify --json` for structured output
- **Agent tools**: Parse JSON to show verification status in UIs
- **Watch mode**: Combine with `watchexec` or similar

## License

MIT
