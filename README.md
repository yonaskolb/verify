# verify

Verified commits, faster CI. Run checks locally, prove they passed in git, skip them in CI.

## Why verify?

**Trust** — Every commit carries proof that checks passed. You know code was verified *before* it was committed, not just after CI runs. Especially valuable when AI agents are writing code — the commit message itself is a receipt that the work was validated.

**Speed** — CI doesn't re-run what was already proven locally. You save wall-clock time and compute costs, scaling with how many checks you have and how often you push.

### How it does this

- **Standalone** — Works with any project, any language
- **Simple** — One YAML file, one binary
- **Fast** — Written in Rust, BLAKE3 hashing, parallel execution
- **Smart** — Only re-runs checks when relevant files change
- **Succinct** — Only returns the errors from your verifications, not the whole build output of every step
- **Open** — JSON output for integration with other tools

## How It Works

1. You define checks (build, lint, test) in `verify.yaml` with glob patterns for the files each check cares about
2. `verify run` executes unverified checks, hashes the files, and records the results
3. A check stays verified until its files change, its config changes, or a dependency becomes unverified
4. Verification state is embedded in the commit message as a `Verified` line — so CI can skip what's already been proven

### Sharing State with CI

A git hook adds a `Verified` line to each commit message containing a hash of each check's configuration and file state:

```
feat: add profile page

Verified: build:a1b2c3d4,lint:e5f6a7b8,tests:c9d0e1f2
```

CI reads this line and compares the hashes against the current files. If they match, the check is skipped.

```bash
# Pre-commit hook — run checks
verify run

# Commit-msg hook — embed verification proof in the commit message
verify sign "$1"

# CI — validate the commit's verification proof against current files
verify check
```

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
verify         # Run unverified checks
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
| `command` | No | Shell command to execute. If omitted, creates an aggregate check whose status is derived from its dependencies |
| `cache_paths` | No | Glob patterns for files that affect this check. If omitted, check is untracked (always runs) |
| `depends_on` | No | List of checks or subprojects that must pass first |
| `metadata` | No | Regex patterns for extracting metrics from output |
| `per_file` | No | Run command once per changed file (sets `VERIFY_FILE` env var) |

### Aggregate Checks

Create checks without a command to group related checks. Their status is derived from their dependencies:

```yaml
verifications:
  - name: build
    command: npm run build
    cache_paths: ["src/**/*.ts"]

  - name: test
    command: npm test
    cache_paths: ["src/**/*.ts", "tests/**/*.ts"]

  - name: all
    depends_on: [build, test]  # verified when both deps are verified
```

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
verify status             # Show all checks
verify status build       # Show status for a specific check
verify status --verify    # Exit with code 1 if any check is unverified
```

Output:
```
● build - verified
● typecheck - verified
● test - unverified (depends on: build)
● lint - unverified (3 file(s) changed)
● e2e - unverified (config changed)
● integration - unverified (never run)
● always-run - untracked
```

### Run Checks

```bash
verify                    # Run all unverified checks
verify run build          # Run specific check (and dependencies)
verify run --force        # Force run even if verified
verify run --verbose      # Stream command output in real-time
```

### Commit Verification

```bash
verify hash              # Print combined hashes for all checks (full 64-char blake3)
verify hash build        # Print hash for a specific check
verify sign FILE         # Embed verification proof in a commit message file
verify check             # Validate the current commit's proof against current files
verify check build       # Validate a specific check
verify sync              # Seed local cache from a Verified trailer in recent git history
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
      "status": "verified"
    },
    {
      "name": "test",
      "status": "unverified",
      "reason": "dependency_unverified",
      "stale_dependency": "build"
    },
    {
      "name": "lint",
      "status": "unverified",
      "reason": "config_changed"
    },
    {
      "name": "always-run",
      "status": "untracked"
    }
  ]
}
```

### Clear Cache

```bash
verify clean           # Clear all cached results (resets verify.lock)
verify clean build     # Clear specific check
```

## Setup

Add `verify.lock` to `.gitignore` (it's a local cache):

```bash
echo "verify.lock" >> .gitignore
```

Set up git hooks to automatically run checks and embed proof in each commit:

```bash
#!/bin/sh
# .git/hooks/pre-commit
verify run

#!/bin/sh
# .git/hooks/commit-msg
verify sign "$1"
```

In CI, validate that the commit's checks match the current file state:

```bash
verify check             # exits 0 if proof matches, 1 if not
verify check tests       # validate a specific check
```

### Syncing Cache in New Worktrees

In a fresh worktree or checkout, `verify sync` bootstraps the local cache from git history:

```bash
verify sync
```

This searches recent commits for a `Verified` trailer, compares the hashes against the current file state, and seeds `verify.lock` with any matching checks. Subsequent `verify run` calls will skip those checks.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | All checks passed (or skipped as verified) |
| 1 | One or more checks failed |
| 2 | Configuration error |

## License

MIT
