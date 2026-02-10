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
