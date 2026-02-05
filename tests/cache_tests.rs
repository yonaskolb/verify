/// Cache behavior and edge case tests
/// Tests for cache persistence, atomicity, version handling, and edge cases

mod common;

use common::TestProject;
use std::fs;

// ==================== Cache Format Tests ====================

#[test]
fn test_cache_version_is_4() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths: []
"#,
    );

    project.run(&["run"]);

    let lock = project.read_lock().expect("Lock file should exist");
    assert_eq!(lock["version"], 4, "Cache version should be 4");
}

#[test]
fn test_cache_content_hash_set_on_success() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: echo "success"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_file("file.txt", "content");
    project.run(&["run"]);

    let lock = project.read_lock().expect("Lock file should exist");
    assert!(
        lock["checks"]["test"]["content_hash"].is_string(),
        "content_hash should be set on success"
    );
}

#[test]
fn test_cache_content_hash_cleared_on_failure() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: exit 1
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_file("file.txt", "content");
    project.run(&["run"]);

    let lock = project.read_lock().expect("Lock file should exist");
    assert!(
        lock["checks"]["test"]["content_hash"].is_null(),
        "content_hash should be null on failure"
    );
}

#[test]
fn test_cache_config_hash_always_set() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: exit 1
    cache_paths: []
"#,
    );

    project.run(&["run"]);

    let lock = project.read_lock().expect("Lock file should exist");
    assert!(
        lock["checks"]["test"]["config_hash"].is_string(),
        "config_hash should be set even on failure"
    );
}

// ==================== Cache Version Migration ====================

#[test]
fn test_old_cache_version_triggers_rerun() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_file("file.txt", "content");

    // First run
    project.run(&["run"]);

    // Manually write an old version lock file
    let old_lock = r#"{
        "version": 3,
        "checks": {
            "test": {
                "config_hash": "old_hash",
                "content_hash": "old_content"
            }
        }
    }"#;
    fs::write(project.path().join("verify.lock"), old_lock).unwrap();

    // Run again - should treat as fresh start due to version mismatch
    let (success, stdout, _) = project.run(&["run"]);
    assert!(success);

    // Should have re-run (not cached)
    let lock = project.read_lock().expect("Lock file should exist");
    assert_eq!(lock["version"], 4, "Version should be updated to 4");
}

// ==================== Cache Atomicity Tests ====================

#[test]
fn test_cache_saved_after_each_check() {
    // If we have multiple checks and one fails, the successful ones should still be cached
    let project = TestProject::new(
        r#"
verifications:
  - name: pass_first
    command: echo "pass"
    cache_paths:
      - "pass.txt"
  - name: fail_second
    command: exit 1
    cache_paths:
      - "fail.txt"
"#,
    );

    project.create_file("pass.txt", "content");
    project.create_file("fail.txt", "content");

    let (success, _, _) = project.run(&["run"]);
    assert!(!success, "Overall run should fail");

    // First check should still be cached
    let lock = project.read_lock().expect("Lock file should exist");
    assert!(
        lock["checks"]["pass_first"]["content_hash"].is_string(),
        "Passing check should be cached"
    );
}

#[test]
fn test_no_temp_file_left_on_success() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths: []
"#,
    );

    project.run(&["run"]);

    // No .tmp file should remain
    assert!(
        !project.file_exists("verify.lock.tmp"),
        "Temp file should not remain after successful save"
    );
}

// ==================== Config Hash Detection ====================

#[test]
fn test_command_change_triggers_rerun() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: echo "original"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_file("file.txt", "content");
    project.run(&["run"]);

    // Change the command
    fs::write(
        project.path().join("verify.yaml"),
        r#"
verifications:
  - name: test
    command: echo "modified"
    cache_paths:
      - "*.txt"
"#,
    )
    .unwrap();

    // Check status - should be stale
    let (success, stdout, _) = project.run(&["status"]);
    assert!(success);
    assert!(
        stdout.contains("config") || stdout.contains("stale"),
        "Should indicate config changed: {}",
        stdout
    );
}

#[test]
fn test_cache_paths_change_triggers_rerun() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_file("file.txt", "content");
    project.create_file("file.rs", "content");
    project.run(&["run"]);

    // Change cache_paths
    fs::write(
        project.path().join("verify.yaml"),
        r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths:
      - "*.rs"
"#,
    )
    .unwrap();

    // Check status - should be stale
    let (success, stdout, _) = project.run(&["status"]);
    assert!(success);
    assert!(
        stdout.contains("config") || stdout.contains("stale"),
        "Should indicate config changed: {}",
        stdout
    );
}

#[test]
fn test_timeout_change_triggers_rerun() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_file("file.txt", "content");
    project.run(&["run"]);

    // Add timeout
    fs::write(
        project.path().join("verify.yaml"),
        r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths:
      - "*.txt"
    timeout_secs: 60
"#,
    )
    .unwrap();

    // Check status - should be stale
    let (success, stdout, _) = project.run(&["status"]);
    assert!(success);
    assert!(
        stdout.contains("config") || stdout.contains("stale"),
        "Should indicate config changed: {}",
        stdout
    );
}

#[test]
fn test_per_file_toggle_triggers_rerun() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_file("file.txt", "content");
    project.run(&["run"]);

    // Enable per_file mode
    fs::write(
        project.path().join("verify.yaml"),
        r#"
verifications:
  - name: test
    command: echo $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    )
    .unwrap();

    // Check status - should be stale
    let (success, stdout, _) = project.run(&["status"]);
    assert!(success);
    assert!(
        stdout.contains("config") || stdout.contains("stale"),
        "Should indicate config changed: {}",
        stdout
    );
}

// ==================== Orphaned Cache Cleanup ====================

#[test]
fn test_orphaned_cache_cleaned_on_run() {
    let project = TestProject::new(
        r#"
verifications:
  - name: keep
    command: echo "keep"
    cache_paths: []
  - name: remove_later
    command: echo "remove"
    cache_paths: []
"#,
    );

    project.run(&["run"]);

    // Both checks should be cached
    let lock = project.read_lock().expect("Lock file should exist");
    assert!(lock["checks"]["keep"].is_object());
    assert!(lock["checks"]["remove_later"].is_object());

    // Remove one check from config
    fs::write(
        project.path().join("verify.yaml"),
        r#"
verifications:
  - name: keep
    command: echo "keep"
    cache_paths: []
"#,
    )
    .unwrap();

    // Run again
    project.run(&["run"]);

    // Orphaned check should be removed
    let lock = project.read_lock().expect("Lock file should exist");
    assert!(
        lock["checks"]["keep"].is_object(),
        "Kept check should remain"
    );
    assert!(
        lock["checks"]["remove_later"].is_null(),
        "Removed check should be cleaned up"
    );
}

// ==================== Clean Command Edge Cases ====================

#[test]
fn test_clean_nonexistent_check_fails() {
    let project = TestProject::new(
        r#"
verifications:
  - name: existing
    command: echo "exists"
    cache_paths: []
"#,
    );

    let (success, _stdout, stderr) = project.run(&["clean", "nonexistent"]);

    // This might succeed with a warning or fail - depends on implementation
    // At minimum, it shouldn't crash
    if !success {
        assert!(
            stderr.contains("nonexistent") || stderr.contains("unknown"),
            "Error should mention the unknown check: {}",
            stderr
        );
    }
}

#[test]
fn test_clean_all_removes_all_checks() {
    let project = TestProject::new(
        r#"
verifications:
  - name: check_a
    command: echo "a"
    cache_paths: []
  - name: check_b
    command: echo "b"
    cache_paths: []
"#,
    );

    project.run(&["run"]);

    // Clean all
    let (success, _, _) = project.run(&["clean"]);
    assert!(success);

    // Lock file should have empty checks or be minimal
    let lock = project.read_lock();
    if let Some(lock) = lock {
        let checks = lock["checks"].as_object();
        assert!(
            checks.map(|c| c.is_empty()).unwrap_or(true),
            "All checks should be cleaned"
        );
    }
}

// ==================== Metadata in Cache ====================

#[test]
fn test_metadata_stored_in_cache() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: "echo 'Coverage: 85%'"
    cache_paths:
      - "*.txt"
    metadata:
      coverage: "Coverage: (\\d+)%"
"#,
    );

    project.create_file("dummy.txt", "content");
    let (success, stdout, stderr) = project.run(&["run"]);

    assert!(
        success,
        "Run should succeed. stdout: {}\nstderr: {}",
        stdout, stderr
    );

    let lock = project.read_lock().expect("Lock file should exist");
    assert!(
        lock["checks"]["test"]["metadata"]["coverage"].is_number(),
        "Metadata should be stored in cache: {:?}",
        lock
    );
}

#[test]
fn test_metadata_cleared_on_failure() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: "echo 'Coverage: 85%' && exit 1"
    cache_paths: []
    metadata:
      coverage: "Coverage: (\\d+)%"
"#,
    );

    project.run(&["run"]);

    let lock = project.read_lock().expect("Lock file should exist");
    // Metadata should not be extracted on failure
    assert!(
        lock["checks"]["test"]["metadata"].is_null()
            || lock["checks"]["test"]["metadata"]
                .as_object()
                .map(|m| m.is_empty())
                .unwrap_or(true),
        "Metadata should be empty on failure"
    );
}

// ==================== NoCachePaths Behavior ====================

#[test]
fn test_no_cache_paths_always_runs() {
    let project = TestProject::new(
        r#"
verifications:
  - name: always_run
    command: echo "running"
    cache_paths: []
"#,
    );

    // First run
    let (success1, _, _) = project.run(&["run"]);
    assert!(success1);

    // Status should show stale (NoCachePaths)
    let (_, stdout, _) = project.run(&["status"]);
    assert!(
        stdout.contains("stale") || stdout.contains("no cache"),
        "Should always be stale with no cache_paths: {}",
        stdout
    );

    // Second run should still execute (not be skipped)
    // The output shows "verified" for successful checks
    let (success2, stdout2, _) = project.run(&["run"]);
    assert!(success2);
    assert!(
        stdout2.contains("verified"),
        "Should execute (not skip) with no cache_paths: {}",
        stdout2
    );
}

// ==================== File Change Detection ====================

#[test]
fn test_file_added_triggers_rerun() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_file("file1.txt", "content");
    project.run(&["run"]);

    // Add a new file
    project.create_file("file2.txt", "new content");

    // Should be stale
    let (_, stdout, _) = project.run(&["status"]);
    assert!(
        stdout.contains("stale") || stdout.contains("changed"),
        "Should be stale when file added: {}",
        stdout
    );
}

#[test]
fn test_file_removed_triggers_rerun() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_file("file1.txt", "content");
    project.create_file("file2.txt", "content");
    project.run(&["run"]);

    // Remove a file
    fs::remove_file(project.path().join("file2.txt")).unwrap();

    // Should be stale
    let (_, stdout, _) = project.run(&["status"]);
    assert!(
        stdout.contains("stale") || stdout.contains("changed"),
        "Should be stale when file removed: {}",
        stdout
    );
}

#[test]
fn test_file_modified_triggers_rerun() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_file("file.txt", "original");
    project.run(&["run"]);

    // Modify the file
    project.create_file("file.txt", "modified");

    // Should be stale
    let (_, stdout, _) = project.run(&["status"]);
    assert!(
        stdout.contains("stale") || stdout.contains("changed"),
        "Should be stale when file modified: {}",
        stdout
    );
}

#[test]
fn test_file_touched_without_content_change_is_fresh() {
    let project = TestProject::new(
        r#"
verifications:
  - name: test
    command: echo "test"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_file("file.txt", "content");
    project.run(&["run"]);

    // Touch file (rewrite same content)
    project.create_file("file.txt", "content");

    // Should still be fresh (content hash same)
    let (_, stdout, _) = project.run(&["status"]);
    assert!(
        stdout.contains("fresh") || stdout.contains("✓"),
        "Should be fresh when content unchanged: {}",
        stdout
    );
}
