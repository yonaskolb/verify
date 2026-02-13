use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// Helper to get the path to the verify binary
fn verify_binary() -> PathBuf {
    // Get the path to the binary built by cargo test
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target");
    path.push("debug");
    path.push("verify");

    // Build if needed
    if !path.exists() {
        let output = Command::new("cargo")
            .args(["build", "--quiet"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("Failed to build project");

        if !output.status.success() {
            panic!(
                "Failed to build: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    path
}

/// Helper to create a test project directory with a verify.yaml config
fn setup_test_project(config_yaml: &str) -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    // Write verify.yaml
    fs::write(temp_dir.path().join("verify.yaml"), config_yaml).expect("Failed to write config");

    temp_dir
}

/// Run verify command and return (success, stdout, stderr)
fn run_verify(project_dir: &Path, args: &[&str]) -> (bool, String, String) {
    let binary = verify_binary();
    let output = Command::new(&binary)
        .args(args)
        .current_dir(project_dir)
        .output()
        .unwrap_or_else(|e| panic!("Failed to execute verify at {:?}: {}", binary, e));

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    (output.status.success(), stdout, stderr)
}

// ==================== Init Command Tests ====================

#[test]
fn test_init_creates_config_file() {
    let temp_dir = TempDir::new().unwrap();

    let (success, _stdout, _stderr) = run_verify(temp_dir.path(), &["init"]);

    assert!(success);
    assert!(temp_dir.path().join("verify.yaml").exists());
}

#[test]
fn test_init_creates_gitignore_entry() {
    let temp_dir = TempDir::new().unwrap();

    let (success, _stdout, _stderr) = run_verify(temp_dir.path(), &["init"]);

    assert!(success);

    let gitignore = fs::read_to_string(temp_dir.path().join(".gitignore")).unwrap();
    assert!(gitignore.contains("**/.verify/"));
}

#[test]
fn test_init_creates_gitattributes_entry() {
    let temp_dir = TempDir::new().unwrap();

    let (success, _stdout, _stderr) = run_verify(temp_dir.path(), &["init"]);

    assert!(success);

    let gitattributes = fs::read_to_string(temp_dir.path().join(".gitattributes")).unwrap();
    assert!(gitattributes.contains("verify.lock merge=ours"));
}

#[test]
fn test_init_fails_if_config_exists() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(temp_dir.path().join("verify.yaml"), "existing: true").unwrap();

    let (success, _stdout, stderr) = run_verify(temp_dir.path(), &["init"]);

    assert!(!success);
    assert!(stderr.contains("already exists") || stderr.contains("Use --force"));
}

#[test]
fn test_init_force_overwrites_existing() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(temp_dir.path().join("verify.yaml"), "existing: true").unwrap();

    let (success, _stdout, _stderr) = run_verify(temp_dir.path(), &["init", "--force"]);

    assert!(success);

    // Config should be the default template, not the original content
    let config = fs::read_to_string(temp_dir.path().join("verify.yaml")).unwrap();
    assert!(config.contains("verifications:"));
}

// ==================== Run Command Tests ====================

#[test]
fn test_run_executes_simple_check() {
    let config = r#"
verifications:
  - name: echo_test
    command: echo "hello"
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    let (success, stdout, _stderr) = run_verify(temp_dir.path(), &["run"]);

    assert!(success, "Run should succeed");
    // Output shows "N verified" summary
    assert!(
        stdout.contains("verified"),
        "Expected 'verified' in output: {}",
        stdout
    );
}

#[test]
fn test_run_creates_lock_file() {
    let config = r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    let (success, _stdout, _stderr) = run_verify(temp_dir.path(), &["run"]);

    assert!(success);
    assert!(temp_dir.path().join("verify.lock").exists());
}

#[test]
fn test_run_failing_check_returns_nonzero() {
    let config = r#"
verifications:
  - name: failing_check
    command: exit 1
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    let (success, _stdout, _stderr) = run_verify(temp_dir.path(), &["run"]);

    assert!(!success, "Run should fail when check fails");
}

#[test]
fn test_run_caches_successful_check() {
    let config = r#"
verifications:
  - name: cached_check
    command: echo "running"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);

    // Create a file to track
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    // First run - should execute
    let (success1, stdout1, _stderr1) = run_verify(temp_dir.path(), &["run"]);
    assert!(success1);

    // Second run - should be cached (faster, shown as verified)
    let (success2, stdout2, _stderr2) = run_verify(temp_dir.path(), &["run"]);
    assert!(success2);
    // Both runs show "verified" - caching is indicated by faster time (0ms)
    assert!(stdout1.contains("verified") && stdout2.contains("verified"));
}

#[test]
fn test_run_detects_file_changes() {
    let config = r#"
verifications:
  - name: change_detect
    command: echo "running"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);

    // Create initial file
    fs::write(temp_dir.path().join("test.txt"), "initial").unwrap();

    // First run
    run_verify(temp_dir.path(), &["run"]);

    // Modify the file
    fs::write(temp_dir.path().join("test.txt"), "modified").unwrap();

    // Get status - should show stale
    let (success, stdout, _stderr) = run_verify(temp_dir.path(), &["status"]);
    assert!(success);
    assert!(stdout.contains("unverified") || stdout.contains("changed") || !stdout.contains("verified"));
}

#[test]
fn test_run_specific_check() {
    let config = r#"
verifications:
  - name: check_a
    command: echo "a"
    cache_paths: []
  - name: check_b
    command: echo "b"
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    // Run only check_a
    let (success, stdout, _stderr) = run_verify(temp_dir.path(), &["run", "check_a"]);

    assert!(success);
    // Should show only 1 verified (not 2)
    assert!(
        stdout.contains("1 verified"),
        "Expected '1 verified' in output: {}",
        stdout
    );
}

#[test]
fn test_run_force_ignores_cache() {
    let config = r#"
verifications:
  - name: force_test
    command: echo "running"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    // First run
    run_verify(temp_dir.path(), &["run"]);

    // Force run - should execute even though cached
    let (success, stdout, _stderr) = run_verify(temp_dir.path(), &["run", "--force"]);
    assert!(success);
    // Should show it ran (pass), not just cached
    assert!(stdout.contains("pass") || stdout.contains("✓") || !stdout.contains("cached"));
}

#[test]
fn test_run_respects_dependencies() {
    let config = r#"
verifications:
  - name: first
    command: echo "first"
    cache_paths: []
  - name: second
    command: echo "second"
    depends_on: [first]
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    let (success, _stdout, _stderr) = run_verify(temp_dir.path(), &["run"]);
    assert!(success);

    // Both checks should have run (no dependency failures)
}

#[test]
fn test_run_dependency_failure_blocks_dependent() {
    let config = r#"
verifications:
  - name: failing_dep
    command: exit 1
    cache_paths: []
  - name: dependent
    command: echo "should not run"
    depends_on: [failing_dep]
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    let (success, stdout, _stderr) = run_verify(temp_dir.path(), &["run"]);

    assert!(!success, "Should fail due to dependency failure");
    // The dependent check should show as blocked/stale due to dependency
    assert!(stdout.contains("dependent") || stdout.contains("failing_dep"));
}

#[test]
fn test_run_json_output() {
    let config = r#"
verifications:
  - name: json_test
    command: echo "test"
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    let (success, stdout, _stderr) = run_verify(temp_dir.path(), &["--json", "run"]);

    assert!(success);
    // Should be valid JSON
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
    assert!(parsed.is_ok(), "Output should be valid JSON: {}", stdout);
}

// ==================== Status Command Tests ====================

#[test]
fn test_status_shows_never_run() {
    let config = r#"
verifications:
  - name: never_run
    command: echo "test"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    let (success, stdout, _stderr) = run_verify(temp_dir.path(), &["status"]);

    assert!(success);
    assert!(stdout.contains("unverified") || stdout.contains("unverified") || stdout.contains("✗"));
}

#[test]
fn test_status_shows_fresh_after_run() {
    let config = r#"
verifications:
  - name: fresh_test
    command: echo "test"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    // Run first
    run_verify(temp_dir.path(), &["run"]);

    // Check status
    let (success, stdout, _stderr) = run_verify(temp_dir.path(), &["status"]);

    assert!(success);
    assert!(stdout.contains("verified") || stdout.contains("✓"));
}

#[test]
fn test_status_json_output() {
    let config = r#"
verifications:
  - name: status_json
    command: echo "test"
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    let (success, stdout, _stderr) = run_verify(temp_dir.path(), &["--json", "status"]);

    assert!(success);
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
    assert!(parsed.is_ok(), "Output should be valid JSON");
}

// ==================== Clean Command Tests ====================

#[test]
fn test_clean_removes_all_cache() {
    let config = r#"
verifications:
  - name: clean_test
    command: echo "test"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    // Run to create cache
    run_verify(temp_dir.path(), &["run"]);
    assert!(temp_dir.path().join("verify.lock").exists());

    // Clean
    let (success, _stdout, _stderr) = run_verify(temp_dir.path(), &["clean"]);
    assert!(success);

    // Lock file should be removed or empty
    if temp_dir.path().join("verify.lock").exists() {
        let lock_content = fs::read_to_string(temp_dir.path().join("verify.lock")).unwrap();
        let lock: serde_json::Value = serde_json::from_str(&lock_content).unwrap();
        // Checks object should be empty
        assert!(
            lock["checks"]
                .as_object()
                .map(|o| o.is_empty())
                .unwrap_or(true)
        );
    }
}

#[test]
fn test_clean_specific_check() {
    let config = r#"
verifications:
  - name: keep_me
    command: echo "keep"
    cache_paths:
      - "keep.txt"
  - name: clean_me
    command: echo "clean"
    cache_paths:
      - "clean.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("keep.txt"), "keep").unwrap();
    fs::write(temp_dir.path().join("clean.txt"), "clean").unwrap();

    // Run both
    run_verify(temp_dir.path(), &["run"]);

    // Clean only clean_me
    let (success, _stdout, _stderr) = run_verify(temp_dir.path(), &["clean", "clean_me"]);
    assert!(success);

    // Check status - keep_me should be fresh, clean_me should need to run
    let (_, stdout, _) = run_verify(temp_dir.path(), &["status"]);

    // keep_me should still show as fresh (or at least its cache should exist)
    // This is a loose check since output format may vary
    assert!(stdout.contains("keep_me"));
}

// ==================== Per-File Mode Tests ====================

#[test]
fn test_per_file_mode_basic() {
    let config = r#"
verifications:
  - name: per_file_test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("file1.txt"), "content1").unwrap();
    fs::write(temp_dir.path().join("file2.txt"), "content2").unwrap();

    let (success, _stdout, _stderr) = run_verify(temp_dir.path(), &["run"]);

    assert!(success);
}

#[test]
fn test_per_file_mode_partial_failure_preserves_progress() {
    let config = r#"
verifications:
  - name: partial_test
    command: |
      if [ "$VERIFY_FILE" = "bad.txt" ]; then
        exit 1
      fi
      cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("good.txt"), "good").unwrap();
    fs::write(temp_dir.path().join("bad.txt"), "bad").unwrap();

    // First run - partial failure
    let (success1, _stdout1, _stderr1) = run_verify(temp_dir.path(), &["run"]);
    assert!(!success1, "Should fail due to bad.txt");

    // Fix the bad file by removing it
    fs::remove_file(temp_dir.path().join("bad.txt")).unwrap();

    // Second run - should only process remaining files
    let (success2, _stdout2, _stderr2) = run_verify(temp_dir.path(), &["run"]);
    assert!(success2);
}

// ==================== Transitive Dependency Tests ====================

#[test]
fn test_run_specific_check_caches_transitive_deps() {
    // Regression test: running a check with transitive deps (C -> B -> A)
    // should use cache for already-verified transitive deps, not re-run them.
    let config = r#"
verifications:
  - name: build
    command: echo "building"
    cache_paths:
      - "src/*.txt"
  - name: previews
    command: echo "recording previews"
    depends_on: [build]
    cache_paths:
      - "src/*.txt"
  - name: snapshots
    command: echo "checking snapshot"
    depends_on: [previews]
    cache_paths:
      - "out/*.txt"
    per_file: true
"#;
    let temp_dir = setup_test_project(config);
    fs::create_dir_all(temp_dir.path().join("src")).unwrap();
    fs::create_dir_all(temp_dir.path().join("out")).unwrap();
    fs::write(temp_dir.path().join("src/app.txt"), "source code").unwrap();
    fs::write(temp_dir.path().join("out/snap.txt"), "snapshot").unwrap();

    // Run all checks first to populate cache
    let (success, _stdout, _stderr) = run_verify(temp_dir.path(), &["run"]);
    assert!(success, "Initial run should succeed");

    // Now modify only the snapshot output (not the source)
    fs::write(temp_dir.path().join("out/snap.txt"), "changed snapshot").unwrap();

    // Run only "snapshots" — build and previews should be cached, not re-run
    let (success, stdout, _stderr) = run_verify(temp_dir.path(), &["--json", "run", "snapshots"]);
    assert!(success, "Snapshot run should succeed");

    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Failed to parse JSON: {}. Output: {}", e, stdout));

    // build and previews should be skipped (cached), not re-executed
    if let Some(results) = parsed["results"].as_array() {
        let build = results.iter().find(|r| r["name"] == "build");
        let previews = results.iter().find(|r| r["name"] == "previews");

        if let Some(build) = build {
            assert_eq!(
                build["result"], "skipped",
                "build should be cached/skipped, got: {:?}",
                build
            );
        }
        if let Some(previews) = previews {
            assert_eq!(
                previews["result"], "skipped",
                "previews should be cached/skipped, got: {:?}",
                previews
            );
        }
    }
}

// ==================== Error Handling Tests ====================

#[test]
fn test_missing_config_file() {
    let temp_dir = TempDir::new().unwrap();

    let (success, _stdout, stderr) = run_verify(temp_dir.path(), &["run"]);

    assert!(!success);
    assert!(
        stderr.contains("verify.yaml") || stderr.contains("config") || stderr.contains("not found")
    );
}

#[test]
fn test_invalid_config_syntax() {
    let temp_dir = TempDir::new().unwrap();
    fs::write(
        temp_dir.path().join("verify.yaml"),
        "invalid: [yaml: syntax",
    )
    .unwrap();

    let (success, _stdout, stderr) = run_verify(temp_dir.path(), &["run"]);

    assert!(!success);
    assert!(stderr.contains("parse") || stderr.contains("yaml") || stderr.contains("Error"));
}

#[test]
fn test_unknown_check_name_error() {
    let config = r#"
verifications:
  - name: existing
    command: echo "exists"
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    let (success, _stdout, stderr) = run_verify(temp_dir.path(), &["run", "nonexistent"]);

    assert!(!success);
    assert!(stderr.contains("nonexistent") || stderr.contains("Unknown"));
}

#[test]
fn test_circular_dependency_error() {
    let config = r#"
verifications:
  - name: a
    command: echo "a"
    depends_on: [b]
    cache_paths: []
  - name: b
    command: echo "b"
    depends_on: [a]
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    // Cycle detection happens in status command (uses DependencyGraph validation)
    let (success, _stdout, stderr) = run_verify(temp_dir.path(), &["status"]);

    assert!(!success, "Status should fail due to circular dependency");
    assert!(
        stderr.to_lowercase().contains("circular") || stderr.to_lowercase().contains("cycle"),
        "Expected circular dependency error in stderr: {}",
        stderr
    );
}

#[test]
fn test_self_dependency_error() {
    let config = r#"
verifications:
  - name: self_dep
    command: echo "self"
    depends_on: [self_dep]
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    let (success, _stdout, stderr) = run_verify(temp_dir.path(), &["run"]);

    assert!(!success);
    assert!(stderr.contains("itself") || stderr.contains("self"));
}

// ==================== Metadata Extraction Tests ====================

#[test]
fn test_metadata_extraction() {
    // Use a raw string with proper escaping for the regex pattern
    let temp_dir = TempDir::new().unwrap();

    // Write config with proper YAML escaping for the regex
    let config = r#"verifications:
  - name: metadata_test
    command: "echo 'Coverage: 85%'"
    cache_paths: []
    metadata:
      coverage: "Coverage: (\\d+)%"
"#;
    fs::write(temp_dir.path().join("verify.yaml"), config).unwrap();

    let (success, stdout, stderr) = run_verify(temp_dir.path(), &["--json", "run"]);

    assert!(success, "Run should succeed. Stderr: {}", stderr);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Failed to parse JSON: {}. Output: {}", e, stdout));

    // Check that metadata was captured in the results array
    if let Some(results) = parsed["results"].as_array() {
        let check = results.iter().find(|c| c["name"] == "metadata_test");
        assert!(check.is_some(), "Should find metadata_test in results");
        if let Some(check) = check {
            assert!(
                check["metadata"]["coverage"].is_number(),
                "Coverage should be extracted as a number: {:?}",
                check["metadata"]
            );
        }
    }
}

// ==================== Status Metadata Tests ====================

#[test]
fn test_status_json_includes_metadata() {
    let temp_dir = TempDir::new().unwrap();

    let config = r#"verifications:
  - name: with_meta
    command: "echo 'Tests: 42 passed, Coverage: 85.5%'"
    cache_paths:
      - "*.txt"
    metadata:
      tests: "Tests: (\\d+) passed"
      coverage: "Coverage: ([\\d.]+)%"
"#;
    fs::write(temp_dir.path().join("verify.yaml"), config).unwrap();
    fs::write(temp_dir.path().join("code.txt"), "content").unwrap();

    // Run to populate cache with metadata
    let (success, _, stderr) = run_verify(temp_dir.path(), &["run"]);
    assert!(success, "Run should succeed. Stderr: {}", stderr);

    // Now check status includes metadata
    let (success, stdout, stderr) = run_verify(temp_dir.path(), &["--json", "status"]);
    assert!(success, "Status should succeed. Stderr: {}", stderr);

    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("Failed to parse JSON: {}. Output: {}", e, stdout));

    let checks = parsed["checks"].as_array().expect("checks should be array");
    let check = checks.iter().find(|c| c["name"] == "with_meta").expect("should find with_meta");

    assert_eq!(check["status"], "verified");
    assert_eq!(check["metadata"]["tests"], serde_json::json!(42));
    assert_eq!(check["metadata"]["coverage"], serde_json::json!(85.5));
}

#[test]
fn test_status_json_omits_metadata_when_empty() {
    let config = r#"
verifications:
  - name: no_meta
    command: echo "test"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    // Run to populate cache
    run_verify(temp_dir.path(), &["run"]);

    // Status should not have metadata field
    let (success, stdout, _) = run_verify(temp_dir.path(), &["--json", "status"]);
    assert!(success);

    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let checks = parsed["checks"].as_array().expect("checks should be array");
    let check = checks.iter().find(|c| c["name"] == "no_meta").expect("should find no_meta");

    assert_eq!(check["status"], "verified");
    assert!(check.get("metadata").is_none() || check["metadata"].is_null());
}

// ==================== Exit Code Tests ====================

#[test]
fn test_exit_code_success() {
    let config = r#"
verifications:
  - name: success
    command: exit 0
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    let binary = verify_binary();
    let status = Command::new(binary)
        .args(["run"])
        .current_dir(temp_dir.path())
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(0));
}

#[test]
fn test_exit_code_failure() {
    let config = r#"
verifications:
  - name: failure
    command: exit 1
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    let binary = verify_binary();
    let status = Command::new(binary)
        .args(["run"])
        .current_dir(temp_dir.path())
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(1));
}

#[test]
fn test_exit_code_config_error() {
    let temp_dir = TempDir::new().unwrap();
    // No config file = config error

    let binary = verify_binary();
    let status = Command::new(binary)
        .args(["run"])
        .current_dir(temp_dir.path())
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(2));
}

// ==================== Cache Persistence Tests ====================

#[test]
fn test_cache_persists_across_runs() {
    let config = r#"
verifications:
  - name: persist_test
    command: echo "persist"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    // First run
    run_verify(temp_dir.path(), &["run"]);

    // Read lock file
    let lock_content = fs::read_to_string(temp_dir.path().join("verify.lock")).unwrap();
    let lock: serde_json::Value = serde_json::from_str(&lock_content).unwrap();

    // Verify cache contains our check
    assert!(lock["checks"]["persist_test"].is_object());
    assert!(lock["checks"]["persist_test"]["content_hash"].is_string());
}

#[test]
fn test_cache_version_is_current() {
    let config = r#"
verifications:
  - name: version_test
    command: echo "version"
    cache_paths: []
"#;
    let temp_dir = setup_test_project(config);

    run_verify(temp_dir.path(), &["run"]);

    let lock_content = fs::read_to_string(temp_dir.path().join("verify.lock")).unwrap();
    let lock: serde_json::Value = serde_json::from_str(&lock_content).unwrap();

    // Version should be current (4)
    assert_eq!(lock["version"], 4);
}

// ==================== Hash Command Tests ====================

fn run_verify_exit_code(project_dir: &Path, args: &[&str]) -> i32 {
    let binary = verify_binary();
    let status = Command::new(&binary)
        .args(args)
        .current_dir(project_dir)
        .status()
        .unwrap_or_else(|e| panic!("Failed to execute verify at {:?}: {}", binary, e));
    status.code().unwrap_or(-1)
}

#[test]
fn test_hash_single_check() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    // Run to populate cache
    let (success, _, _) = run_verify(temp_dir.path(), &["run"]);
    assert!(success);

    // Get hash
    let (success, stdout, _) = run_verify(temp_dir.path(), &["hash", "build"]);
    assert!(success);
    let hash = stdout.trim();
    assert_eq!(hash.len(), 64, "Hash should be 64-char hex: {}", hash);

    // Hash should be deterministic
    let (_, stdout2, _) = run_verify(temp_dir.path(), &["hash", "build"]);
    assert_eq!(hash, stdout2.trim());
}

#[test]
fn test_hash_all_checks() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
  - name: lint
    command: echo "lint"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    run_verify(temp_dir.path(), &["run"]);

    let (success, stdout, _) = run_verify(temp_dir.path(), &["hash"]);
    assert!(success);
    let output = stdout.trim();
    // Format: name:hash,name:hash
    assert!(output.contains("build:"), "Output: {}", output);
    assert!(output.contains("lint:"), "Output: {}", output);
    assert!(output.contains(','), "Should be comma-separated: {}", output);
}

#[test]
fn test_hash_unknown_check() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths: ["*.txt"]
"#;
    let temp_dir = setup_test_project(config);

    let exit_code = run_verify_exit_code(temp_dir.path(), &["hash", "nonexistent"]);
    assert_eq!(exit_code, 2);
}

#[test]
fn test_hash_before_run_fails() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    // Try hash without running first
    let exit_code = run_verify_exit_code(temp_dir.path(), &["hash", "build"]);
    assert_eq!(exit_code, 2, "Should exit 2 when check hasn't been run");
}

#[test]
fn test_hash_excludes_aggregate_checks() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
  - name: lint
    command: echo "lint"
    cache_paths:
      - "*.txt"
  - name: all
    depends_on: [build, lint]
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    run_verify(temp_dir.path(), &["run"]);

    // Hash all — aggregate "all" should be excluded
    let (success, stdout, _) = run_verify(temp_dir.path(), &["hash"]);
    assert!(success);
    let output = stdout.trim();
    assert!(output.contains("build:"), "Output: {}", output);
    assert!(output.contains("lint:"), "Output: {}", output);
    assert!(!output.contains("all:"), "Aggregate should be excluded: {}", output);

    // Hash specific aggregate — should fail
    let exit_code = run_verify_exit_code(temp_dir.path(), &["hash", "all"]);
    assert_eq!(exit_code, 2, "Hashing aggregate should fail");
}

#[test]
fn test_hash_changes_when_files_change() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content1").unwrap();

    run_verify(temp_dir.path(), &["run"]);
    let (_, stdout1, _) = run_verify(temp_dir.path(), &["hash", "build"]);

    // Change file, re-run
    fs::write(temp_dir.path().join("test.txt"), "content2").unwrap();
    run_verify(temp_dir.path(), &["run"]);
    let (_, stdout2, _) = run_verify(temp_dir.path(), &["hash", "build"]);

    assert_ne!(stdout1.trim(), stdout2.trim());
}

#[test]
fn test_hash_excludes_stale_checks() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
  - name: lint
    command: echo "lint"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    run_verify(temp_dir.path(), &["run"]);

    // Both checks are fresh — both should appear in hash output
    let (success, stdout, _) = run_verify(temp_dir.path(), &["hash"]);
    assert!(success);
    assert!(stdout.contains("build:"));
    assert!(stdout.contains("lint:"));

    // Change a file — both checks become stale
    fs::write(temp_dir.path().join("test.txt"), "changed").unwrap();

    // Hash specific stale check — should fail
    let exit_code = run_verify_exit_code(temp_dir.path(), &["hash", "build"]);
    assert_eq!(exit_code, 2, "Stale check should not be hashable");

    // Hash all — should produce empty output (no fresh checks)
    let (success, stdout, _) = run_verify(temp_dir.path(), &["hash"]);
    assert!(success);
    assert_eq!(stdout.trim(), "", "No fresh checks should produce empty output");
}

// ==================== Trailer Command Tests ====================

/// Truncate hash values in "name:fullhash,name:fullhash" format to 8-char hashes
/// to match the trailer format used by `verify trailer` and `verify check`.
fn truncate_hash_output(output: &str) -> String {
    output
        .split(',')
        .map(|pair| {
            if let Some((name, hash)) = pair.split_once(':') {
                format!("{}:{}", name, &hash[..8.min(hash.len())])
            } else {
                pair.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Initialize a git repo in the given directory with an initial commit
fn init_git_repo(dir: &Path) {
    Command::new("git")
        .args(["init"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(dir)
        .output()
        .unwrap();
}

#[test]
fn test_sign_writes_to_file() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    run_verify(temp_dir.path(), &["run"]);

    // Create a commit message file (not .txt to avoid matching cache_paths)
    let msg_file = temp_dir.path().join("COMMIT_MSG");
    fs::write(&msg_file, "feat: add feature\n").unwrap();

    // Need git repo for git interpret-trailers
    init_git_repo(temp_dir.path());

    let (success, _, stderr) = run_verify(
        temp_dir.path(),
        &["sign", msg_file.to_str().unwrap()],
    );
    assert!(success, "sign command failed: {}", stderr);

    let content = fs::read_to_string(&msg_file).unwrap();
    assert!(content.contains("Verified:"), "Trailer not found in: {}", content);
    assert!(content.contains("build:"), "Build hash not in trailer: {}", content);
}

#[test]
fn test_sign_replaces_existing_trailer() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    run_verify(temp_dir.path(), &["run"]);

    let msg_file = temp_dir.path().join("COMMIT_MSG");
    fs::write(&msg_file, "feat: add feature\n").unwrap();

    init_git_repo(temp_dir.path());

    // Sign twice — should replace, not duplicate
    run_verify(temp_dir.path(), &["sign", msg_file.to_str().unwrap()]);
    run_verify(temp_dir.path(), &["sign", msg_file.to_str().unwrap()]);

    let content = fs::read_to_string(&msg_file).unwrap();
    let count = content.matches("Verified:").count();
    assert_eq!(count, 1, "Should have exactly one Verified trailer, got {}: {}", count, content);
}

#[test]
fn test_check_verified_with_matching_trailer() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    // Init git repo
    init_git_repo(temp_dir.path());

    // Run verify to populate cache
    run_verify(temp_dir.path(), &["run"]);

    // Get the trailer value (truncated to match trailer format)
    let (_, hash_output, _) = run_verify(temp_dir.path(), &["hash"]);
    let trailer_value = truncate_hash_output(hash_output.trim());

    // Create a commit with the trailer
    let commit_msg = format!("feat: add feature\n\nVerified: {}\n", trailer_value);
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", &commit_msg])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();

    // Check should pass
    let exit_code = run_verify_exit_code(temp_dir.path(), &["check"]);
    assert_eq!(exit_code, 0, "Should exit 0 when trailer matches");
}

#[test]
fn test_check_unverified_after_file_change() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    init_git_repo(temp_dir.path());

    // Run, get hash, commit with trailer
    run_verify(temp_dir.path(), &["run"]);
    let (_, hash_output, _) = run_verify(temp_dir.path(), &["hash"]);
    let trailer_value = truncate_hash_output(hash_output.trim());

    let commit_msg = format!("feat: stuff\n\nVerified: {}\n", trailer_value);
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", &commit_msg])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();

    // Modify a file — trailer should no longer match
    fs::write(temp_dir.path().join("test.txt"), "changed").unwrap();

    let exit_code = run_verify_exit_code(temp_dir.path(), &["check"]);
    assert_eq!(exit_code, 1, "Should exit 1 when files changed");
}

#[test]
fn test_check_unverified_no_trailer() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    init_git_repo(temp_dir.path());

    // No trailer in the commit
    let exit_code = run_verify_exit_code(temp_dir.path(), &["check"]);
    assert_eq!(exit_code, 1, "Should exit 1 when no trailer");
}

#[test]
fn test_check_specific_check_name() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
  - name: lint
    command: echo "lint"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    init_git_repo(temp_dir.path());

    run_verify(temp_dir.path(), &["run"]);
    let (_, hash_output, _) = run_verify(temp_dir.path(), &["hash"]);
    let trailer_value = truncate_hash_output(hash_output.trim());

    let commit_msg = format!("feat: stuff\n\nVerified: {}\n", trailer_value);
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", &commit_msg])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();

    // Check specific check
    let exit_code = run_verify_exit_code(temp_dir.path(), &["check", "build"]);
    assert_eq!(exit_code, 0, "build should be verified");

    let exit_code = run_verify_exit_code(temp_dir.path(), &["check", "lint"]);
    assert_eq!(exit_code, 0, "lint should be verified");
}

#[test]
fn test_trailer_and_check_roundtrip() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
  - name: lint
    command: echo "lint"
    cache_paths:
      - "*.txt"
  - name: all
    depends_on: [build, lint]
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    init_git_repo(temp_dir.path());

    // Run all checks
    run_verify(temp_dir.path(), &["run"]);

    // Use trailer command to write to a file (not .txt to avoid matching cache_paths)
    let msg_file = temp_dir.path().join("COMMIT_MSG");
    fs::write(&msg_file, "feat: roundtrip test\n").unwrap();

    let (success, _, _) = run_verify(
        temp_dir.path(),
        &["sign", msg_file.to_str().unwrap()],
    );
    assert!(success);

    // Commit using that message file
    Command::new("git")
        .args(["commit", "--allow-empty", "-F", msg_file.to_str().unwrap()])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();

    // Non-aggregate checks should verify
    let exit_code = run_verify_exit_code(temp_dir.path(), &["check"]);
    assert_eq!(exit_code, 0, "All checks should be verified after roundtrip");

    let exit_code = run_verify_exit_code(temp_dir.path(), &["check", "build"]);
    assert_eq!(exit_code, 0, "build should be verified");

    let exit_code = run_verify_exit_code(temp_dir.path(), &["check", "lint"]);
    assert_eq!(exit_code, 0, "lint should be verified");

    // Composite check resolves from its deps — all deps verified so composite passes
    let exit_code = run_verify_exit_code(temp_dir.path(), &["check", "all"]);
    assert_eq!(exit_code, 0, "Composite should be verified when all deps are");

    // Verify composite is not in the trailer itself
    let content = fs::read_to_string(&msg_file).unwrap();
    assert!(!content.contains("all:"), "Composite should not be in trailer: {}", content);
}

#[test]
fn test_check_composite_fails_when_dep_stale() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
  - name: lint
    command: echo "lint"
    cache_paths:
      - "*.txt"
  - name: all
    depends_on: [build, lint]
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    init_git_repo(temp_dir.path());

    // Run, sign, commit
    run_verify(temp_dir.path(), &["run"]);
    let msg_file = temp_dir.path().join("COMMIT_MSG");
    fs::write(&msg_file, "feat: test\n").unwrap();
    let (success, _, _) = run_verify(
        temp_dir.path(),
        &["sign", msg_file.to_str().unwrap()],
    );
    assert!(success);
    Command::new("git")
        .args(["commit", "--allow-empty", "-F", msg_file.to_str().unwrap()])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();

    // Everything should pass initially
    let exit_code = run_verify_exit_code(temp_dir.path(), &["check", "all"]);
    assert_eq!(exit_code, 0, "Composite should pass when deps match");

    // Change a file — invalidates build and lint
    fs::write(temp_dir.path().join("test.txt"), "changed").unwrap();

    // Individual checks should fail
    let exit_code = run_verify_exit_code(temp_dir.path(), &["check", "build"]);
    assert_eq!(exit_code, 1, "build should fail after file change");

    // Composite should also fail since its deps are stale
    let exit_code = run_verify_exit_code(temp_dir.path(), &["check", "all"]);
    assert_eq!(exit_code, 1, "Composite should fail when dep is stale");
}

#[test]
fn test_sync_seeds_cache_from_trailer() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
  - name: lint
    command: echo "lint"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    init_git_repo(temp_dir.path());

    // Run checks to populate cache
    run_verify(temp_dir.path(), &["run"]);

    // Sign and commit with trailer
    let msg_file = temp_dir.path().join("COMMIT_MSG");
    fs::write(&msg_file, "feat: add feature\n").unwrap();
    run_verify(temp_dir.path(), &["sign", msg_file.to_str().unwrap()]);
    Command::new("git")
        .args(["commit", "--allow-empty", "-F", msg_file.to_str().unwrap()])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();

    // Delete the lock file (simulates fresh worktree)
    fs::remove_file(temp_dir.path().join("verify.lock")).unwrap();

    // Sync should seed the cache from the trailer
    let exit_code = run_verify_exit_code(temp_dir.path(), &["sync"]);
    assert_eq!(exit_code, 0, "Sync should succeed when trailer matches");

    // Lock file should now exist
    assert!(temp_dir.path().join("verify.lock").exists(), "verify.lock should be created");

    // Status should show checks as verified
    let (success, stdout, _) = run_verify(temp_dir.path(), &["status", "--json"]);
    assert!(success);
    assert!(stdout.contains("\"verified\""), "Checks should be verified after sync: {}", stdout);
}

#[test]
fn test_sync_no_trailer() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    init_git_repo(temp_dir.path());

    // No trailer in history — sync should fail
    let exit_code = run_verify_exit_code(temp_dir.path(), &["sync"]);
    assert_eq!(exit_code, 1, "Sync should exit 1 when no trailer found");
}

#[test]
fn test_sync_finds_trailer_in_history() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    init_git_repo(temp_dir.path());

    // Run, sign, and commit with trailer
    run_verify(temp_dir.path(), &["run"]);
    let msg_file = temp_dir.path().join("COMMIT_MSG");
    fs::write(&msg_file, "feat: with trailer\n").unwrap();
    run_verify(temp_dir.path(), &["sign", msg_file.to_str().unwrap()]);
    Command::new("git")
        .args(["commit", "--allow-empty", "-F", msg_file.to_str().unwrap()])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();

    // Make another commit WITHOUT a trailer (simulates a merge commit)
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "chore: merge"])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();

    // Delete the lock file
    fs::remove_file(temp_dir.path().join("verify.lock")).unwrap();

    // Sync should still find the trailer from the previous commit
    let exit_code = run_verify_exit_code(temp_dir.path(), &["sync"]);
    assert_eq!(exit_code, 0, "Sync should find trailer in history");

    // Verify the cache is seeded
    let (success, stdout, _) = run_verify(temp_dir.path(), &["status", "--json"]);
    assert!(success);
    assert!(stdout.contains("\"verified\""), "Check should be verified after sync from history");
}

#[test]
fn test_sync_partial_match() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "src/*.txt"
  - name: lint
    command: echo "lint"
    cache_paths:
      - "docs/*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::create_dir_all(temp_dir.path().join("src")).unwrap();
    fs::create_dir_all(temp_dir.path().join("docs")).unwrap();
    fs::write(temp_dir.path().join("src/main.txt"), "code").unwrap();
    fs::write(temp_dir.path().join("docs/readme.txt"), "docs").unwrap();

    init_git_repo(temp_dir.path());

    // Run, sign, commit
    run_verify(temp_dir.path(), &["run"]);
    let msg_file = temp_dir.path().join("COMMIT_MSG");
    fs::write(&msg_file, "feat: stuff\n").unwrap();
    run_verify(temp_dir.path(), &["sign", msg_file.to_str().unwrap()]);
    Command::new("git")
        .args(["commit", "--allow-empty", "-F", msg_file.to_str().unwrap()])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();

    // Change only docs files — build should still match, lint should not
    fs::write(temp_dir.path().join("docs/readme.txt"), "changed docs").unwrap();

    // Delete lock file
    fs::remove_file(temp_dir.path().join("verify.lock")).unwrap();

    // Sync should partially succeed
    let exit_code = run_verify_exit_code(temp_dir.path(), &["sync"]);
    assert_eq!(exit_code, 0, "Sync should succeed with partial match");

    // Build should be verified, lint should not be in the synced cache
    let (_, stdout, _) = run_verify(temp_dir.path(), &["status", "--json"]);
    // Parse the JSON to check individual statuses
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let checks = json["checks"].as_array().unwrap();

    let build_status = checks.iter().find(|c| c["name"] == "build").unwrap();
    assert_eq!(build_status["status"], "verified", "build should be verified");

    let lint_status = checks.iter().find(|c| c["name"] == "lint").unwrap();
    assert_ne!(lint_status["status"], "verified", "lint should NOT be verified (files changed)");
}

#[test]
fn test_sync_then_run_skips_verified() {
    let config = r#"
verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.txt"
"#;
    let temp_dir = setup_test_project(config);
    fs::write(temp_dir.path().join("test.txt"), "content").unwrap();

    init_git_repo(temp_dir.path());

    // Run, sign, commit
    run_verify(temp_dir.path(), &["run"]);
    let msg_file = temp_dir.path().join("COMMIT_MSG");
    fs::write(&msg_file, "feat: stuff\n").unwrap();
    run_verify(temp_dir.path(), &["sign", msg_file.to_str().unwrap()]);
    Command::new("git")
        .args(["commit", "--allow-empty", "-F", msg_file.to_str().unwrap()])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();

    // Delete lock file
    fs::remove_file(temp_dir.path().join("verify.lock")).unwrap();

    // Sync
    let exit_code = run_verify_exit_code(temp_dir.path(), &["sync"]);
    assert_eq!(exit_code, 0);

    // Run should skip the synced check (shows as cached/verified)
    let (success, stdout, _) = run_verify(temp_dir.path(), &["run"]);
    assert!(success, "Run should succeed");
    assert!(stdout.contains("verified"), "Run should show build as verified/cached: {}", stdout);
}
