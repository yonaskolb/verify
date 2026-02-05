# verify

A fast, lightweight CLI for managing project verification checks with intelligent caching.

## Why verify?

In agent-driven development, verification is key to assuring what gets built is correct. You want it to be fast and accurate. Instead of guiding the agent to run the same custom checks over and over again, and wasting tokens, you can run a single verify command that either gives the green light or provides the error context required to fix the problem. Once those checks are done on one machine, they also don't need to be done on another like CI, speeding up the workflow.

- **Standalone** - Works with any project, any language
- **Simple** - One YAML file, one binary, one lock file
- **Fast** - Written in rust, BLAKE3 hashing, parallel execution
- **Smart** - Only re-runs checks when relevant files change, saving you time.
- **Succinct** - Only returns the errors from your verifications, not the whole build output of every step
- **Open** - Use the json output to integrate into other tools like ui.

## Installation

```bash
curl -fsSL https://raw.githubusercontent.com/yonaskolb/verify/master/install.sh | sh
```

### From source

```bash
cargo install --git https://github.com/yonaskolb/verify
```

Or clone and build:

```bash
git clone https://github.com/yonaskolb/verify
cd verify
cargo build --release
# Binary at ./target/release/verify

# Or install to ~/.cargo/bin
cargo install --path .
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
| `per_file` | No | Run command once per stale file (sets `VERIFY_FILE` env var) |

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

### Per-File Mode

Run a command once for each stale file individually. Useful for test flows, slow operations, or checks that operate on single files:

```yaml
verifications:
  - name: flow-tests
    command: maestro test "$VERIFY_FILE"
    cache_paths:
      - "flows/**/*.yaml"
    per_file: true
```

When `per_file: true`:
- The command runs once per stale file with `VERIFY_FILE` environment variable set to the file path
- Files that haven't changed are skipped (cached)
- Progress shows each file as it runs:
  ```
  ● flow-tests (3 cached)
  ● flow-tests: flows/login.yaml (2.1s)
  ● flow-tests: flows/checkout.yaml (1.8s)
  ```
- If any file fails, execution stops and the error is reported

## Usage

### Check Status

```bash
verify status
```

Output:
```
✓ build - fresh
✓ typecheck - fresh
○ test - stale (depends on: build)
○ lint - stale (3 files changed)
○ e2e - stale (config changed)
? integration - never run
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
      "status": "fresh"
    },
    {
      "name": "test",
      "status": "stale",
      "reason": "dependency_stale",
      "stale_dependency": "build"
    },
    {
      "name": "lint",
      "status": "stale",
      "reason": "config_changed"
    }
  ]
}
```

### Clear Cache

```bash
verify clean           # Clear all cached results (resets verify.lock)
verify clean build     # Clear specific check
```

The cache is stored in `verify.lock` at your project root. This file is designed to be committed to version control.

## How It Works

1. **File Hashing**: verify computes BLAKE3 hashes of all files matching `cache_paths`
2. **Cache Storage**: Results are stored in `verify.lock` at the project root - a committable lock file that travels with your code
3. **Staleness Detection**: A check is stale if:
   - Files in `cache_paths` changed since last successful run
   - The check definition changed in `verify.yaml` (command, cache_paths, timeout, per_file, or metadata patterns)
   - Any dependency (check or subproject) is stale
   - Last run failed
   - No `cache_paths` defined (always runs)
4. **Parallel Execution**: Independent checks run concurrently
5. **Dependency Ordering**: Checks run in topological order respecting `depends_on`
6. **Incremental Saves**: Cache is saved after each check completes (and after each file in per_file mode)

## Shared Cache Across Local and CI

The `verify.lock` file is designed to be committed to version control. This enables a powerful workflow where verification work is done only once:

### The Workflow

1. **Developer runs checks locally**
   ```bash
   verify run
   # All checks pass, verify.lock is updated
   ```

2. **Developer commits verify.lock with their changes**
   ```bash
   git add verify.lock src/
   git commit -m "Add new feature"
   git push
   ```

3. **CI checks out the code including verify.lock**

4. **CI runs verify - skips already-passed checks**
   ```bash
   verify run
   # Checks are fresh because file hashes match what developer already verified
   ```

### Why This Matters

- **No duplicate work**: If you verified locally, CI doesn't re-run the same checks on the same files
- **Faster CI**: Only checks that weren't run locally (or files that changed after local verification) need to run
- **Branch-aware**: Different branches have different lock files, so switching branches re-verifies as needed
- **Worktree-friendly**: Works naturally with git worktrees since verify.lock is per-directory

### Merge Strategy

When you run `verify init`, it automatically adds to `.gitattributes`:

```
verify.lock merge=ours
```

This tells git to prefer "our" version during merges - you'll re-run verify after merging anyway to validate the combined changes.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | All checks passed (or skipped as fresh) |
| 1 | One or more checks failed |
| 2 | Configuration error |

## Git Hooks

Use the `--stage` flag in pre-commit hooks to automatically stage `verify.lock`:

```bash
#!/bin/sh
# .git/hooks/pre-commit
verify --stage
```

This ensures your verification state is included in the commit. The `--stage` flag runs `git add verify.lock` after successful verification.

## Integration Ideas

- **CI/CD**: Commit `verify.lock` to share verification state between local and CI - checks that passed locally won't re-run on CI
- **Agent tools**: Parse JSON output (`verify --json`) to show verification status in UIs
- **Watch mode**: Combine with `watchexec` or similar

## License

MIT
