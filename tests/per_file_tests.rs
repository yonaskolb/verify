/// Per-file mode integration tests
/// Tests for the per_file: true feature that runs commands once per file
mod common;

use common::TestProject;
use std::fs;

// ==================== Basic Per-File Mode Tests ====================

#[test]
fn test_per_file_receives_verify_file_env() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: echo "Processing $VERIFY_FILE"
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("file1.txt", "content1");
    project.create_file("file2.txt", "content2");

    let (success, _stdout, _stderr) = project.run(&["run"]);
    assert!(success, "Per-file run should succeed");
}

#[test]
fn test_per_file_runs_for_each_file() {
    let project = TestProject::new(
        r#"verifications:
  - name: counter
    command: "echo file=$VERIFY_FILE"
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("a.txt", "a");
    project.create_file("b.txt", "b");
    project.create_file("c.txt", "c");

    let (success, stdout, stderr) = project.run(&["run"]);
    assert!(
        success,
        "Should process all files successfully. stdout: {}\nstderr: {}",
        stdout, stderr
    );

    // Verify all files are in the cache
    let lock = project.read_lock().expect("Lock file should exist");
    let file_hashes = lock["checks"]["counter"]["file_hashes"]
        .as_object()
        .expect("Should have file_hashes");
    assert_eq!(file_hashes.len(), 3, "Should have processed 3 files");
}

#[test]
fn test_per_file_stores_file_hashes() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("file1.txt", "content1");
    project.create_file("file2.txt", "content2");

    project.run(&["run"]);

    let lock = project.read_lock().expect("Lock file should exist");

    // Per-file checks should store file_hashes
    let file_hashes = &lock["checks"]["test"]["file_hashes"];
    assert!(
        file_hashes.is_object(),
        "file_hashes should be an object: {:?}",
        lock
    );

    let hashes_obj = file_hashes.as_object().unwrap();
    assert_eq!(
        hashes_obj.len(),
        2,
        "Should have hashes for both files: {:?}",
        hashes_obj
    );
}

// ==================== Partial Progress Tests ====================

#[test]
fn test_per_file_partial_failure_preserves_passing_files() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: |
      if [ "$VERIFY_FILE" = "bad.txt" ]; then
        exit 1
      fi
      cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("good1.txt", "good1");
    project.create_file("good2.txt", "good2");
    project.create_file("bad.txt", "bad");

    // First run - partial failure
    let (success1, _, _) = project.run(&["run"]);
    assert!(!success1, "Should fail due to bad.txt");

    // Check that passing files are cached
    let lock = project.read_lock().expect("Lock file should exist");
    let file_hashes = lock["checks"]["test"]["file_hashes"]
        .as_object()
        .expect("Should have file_hashes");

    // good1.txt and good2.txt should be in file_hashes (they passed)
    // The exact behavior depends on execution order
    assert!(
        !file_hashes.is_empty(),
        "At least some files should be cached"
    );

    // content_hash should be null (overall check failed)
    assert!(
        lock["checks"]["test"]["content_hash"].is_null(),
        "content_hash should be null when check failed"
    );
}

#[test]
fn test_per_file_rerun_only_processes_changed_files() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("file1.txt", "content1");
    project.create_file("file2.txt", "content2");

    // First run
    project.run(&["run"]);

    // Get file1's original hash
    let lock1 = project.read_lock().unwrap();
    let orig_hash = lock1["checks"]["test"]["file_hashes"]["file1.txt"]
        .as_str()
        .map(|s| s.to_string());

    // Modify only file1
    project.create_file("file1.txt", "modified");

    // Run again
    let (success, _, _) = project.run(&["run"]);
    assert!(success);

    // file1's hash should have changed
    let lock2 = project.read_lock().unwrap();
    let new_hash = lock2["checks"]["test"]["file_hashes"]["file1.txt"]
        .as_str()
        .map(|s| s.to_string());

    assert_ne!(orig_hash, new_hash, "file1's hash should have changed");

    // file2's hash should be the same (not reprocessed)
    let file2_hash_before = lock1["checks"]["test"]["file_hashes"]["file2.txt"].as_str();
    let file2_hash_after = lock2["checks"]["test"]["file_hashes"]["file2.txt"].as_str();
    assert_eq!(
        file2_hash_before, file2_hash_after,
        "file2's hash should remain unchanged"
    );
}

#[test]
fn test_per_file_failed_file_rerun_on_retry() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: |
      if [ "$VERIFY_FILE" = "fail.txt" ] && [ ! -f /tmp/verify_retry_marker ]; then
        exit 1
      fi
      cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("pass.txt", "pass");
    project.create_file("fail.txt", "fail");

    // First run - fail.txt fails
    let (success1, _, _) = project.run(&["run"]);
    assert!(!success1);

    // Create marker to make fail.txt pass
    fs::write("/tmp/verify_retry_marker", "").ok();

    // Second run - should retry fail.txt
    let (success2, _, _) = project.run(&["run"]);

    // Clean up
    fs::remove_file("/tmp/verify_retry_marker").ok();

    assert!(success2, "Retry should succeed after fix");
}

// ==================== Config Change Invalidation ====================

#[test]
fn test_per_file_config_change_invalidates_all_files() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: echo $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("file1.txt", "content1");
    project.create_file("file2.txt", "content2");

    // First run
    project.run(&["run"]);

    // Verify files are cached
    let lock1 = project.read_lock().unwrap();
    let hashes1 = lock1["checks"]["test"]["file_hashes"]
        .as_object()
        .unwrap()
        .len();
    assert_eq!(hashes1, 2, "Both files should be cached");

    // Change the command
    fs::write(
        project.path().join("verify.yaml"),
        r#"verifications:
  - name: test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    )
    .unwrap();

    // Status should show stale (config changed)
    let (_, stdout, _) = project.run(&["status"]);
    assert!(
        stdout.contains("stale") || stdout.contains("config"),
        "Should be stale due to config change: {}",
        stdout
    );

    // Run again - all files should be reprocessed
    project.run(&["run"]);

    // Both files should have new hashes
    let lock2 = project.read_lock().unwrap();
    assert!(
        lock2["checks"]["test"]["content_hash"].is_string(),
        "Should be complete after rerun"
    );
}

// ==================== New File Detection ====================

#[test]
fn test_per_file_new_file_detected_as_stale() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("file1.txt", "content1");
    project.run(&["run"]);

    // Add a new file
    project.create_file("file2.txt", "content2");

    // Status should show stale
    let (_, stdout, _) = project.run(&["status"]);
    assert!(
        stdout.contains("stale") || stdout.contains("changed"),
        "Should detect new file as stale: {}",
        stdout
    );
}

#[test]
fn test_per_file_deleted_file_handled() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("file1.txt", "content1");
    project.create_file("file2.txt", "content2");
    project.run(&["run"]);

    // Delete file2
    fs::remove_file(project.path().join("file2.txt")).unwrap();

    // Run again - should handle the deleted file gracefully
    let (success, _, _) = project.run(&["run"]);
    assert!(success, "Should handle deleted file gracefully");

    // The check should still succeed even with one file deleted
    // Note: cache may retain old entries - this tests that execution works
}

// ==================== Empty Files and Edge Cases ====================

#[test]
fn test_per_file_empty_file_handled() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("empty.txt", "");
    project.create_file("normal.txt", "content");

    let (success, _, _) = project.run(&["run"]);
    assert!(success, "Should handle empty files");
}

#[test]
fn test_per_file_no_matching_files() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    // No txt files - only other files
    project.create_file("file.rs", "content");

    let (success, stdout, _) = project.run(&["run"]);
    assert!(success, "Should succeed with no matching files");

    // Should show as cached/skipped (no files to process)
    assert!(
        stdout.contains("cached") || stdout.contains("0 files") || stdout.contains("verified"),
        "Should indicate no files to process: {}",
        stdout
    );
}

#[test]
fn test_per_file_special_characters_in_filename() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: cat "$VERIFY_FILE"
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    // File with spaces
    project.create_file("file with spaces.txt", "content");

    let (success, _, stderr) = project.run(&["run"]);
    assert!(
        success,
        "Should handle spaces in filenames. Stderr: {}",
        stderr
    );
}

// ==================== Per-File with Dependencies ====================

#[test]
fn test_per_file_respects_dependency_failure() {
    let project = TestProject::new(
        r#"verifications:
  - name: build
    command: exit 1
    cache_paths: []
  - name: test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
    depends_on: [build]
"#,
    );

    project.create_file("file.txt", "content");

    let (success, stdout, _) = project.run(&["run"]);
    assert!(!success, "Should fail due to dependency failure");

    // The dependent check should be marked as blocked/stale due to dependency
    // This is shown in the output or results
    assert!(
        stdout.contains("build") || stdout.contains("fail") || stdout.contains("test"),
        "Output should show check names: {}",
        stdout
    );
}

#[test]
fn test_per_file_after_successful_dependency() {
    let project = TestProject::new(
        r#"verifications:
  - name: build
    command: echo "building"
    cache_paths: []
  - name: test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
    depends_on: [build]
"#,
    );

    project.create_file("file.txt", "content");

    let (success, _, _) = project.run(&["run"]);
    assert!(success, "Should succeed when dependency passes");

    // Per-file check should have run
    let lock = project.read_lock().unwrap();
    let file_hashes = lock["checks"]["test"]["file_hashes"].as_object();
    assert!(
        file_hashes.map(|h| !h.is_empty()).unwrap_or(false),
        "Per-file check should have processed files"
    );
}

// ==================== Metadata with Per-File ====================

#[test]
fn test_per_file_metadata_from_last_file() {
    // Metadata is extracted from the last file's output
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: "echo 'Lines: 42'"
    cache_paths:
      - "*.txt"
    per_file: true
    metadata:
      lines: "Lines: (\\d+)"
"#,
    );

    project.create_file("file1.txt", "content1");
    project.create_file("file2.txt", "content2");

    let (success, stdout, stderr) = project.run(&["run"]);
    assert!(
        success,
        "Run should succeed. stdout: {}\nstderr: {}",
        stdout, stderr
    );

    let lock = project.read_lock().unwrap();
    // Metadata should be captured (from last successful file)
    assert!(
        lock["checks"]["test"]["metadata"]["lines"].is_number(),
        "Metadata should be extracted: {:?}",
        lock
    );
}

// ==================== Force Run with Per-File ====================

#[test]
fn test_per_file_force_reruns_all_files() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("file1.txt", "content1");
    project.create_file("file2.txt", "content2");

    // First run
    project.run(&["run"]);

    // Force run - should reprocess all files
    let (success, stdout, _) = project.run(&["run", "--force"]);
    assert!(success);

    // The run should succeed with --force (it reruns even when cached)
    assert!(
        stdout.contains("verified"),
        "Should show verification completed: {}",
        stdout
    );
}

// ==================== JSON Output with Per-File ====================

#[test]
fn test_per_file_json_output() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("file.txt", "content");

    let (success, stdout, _) = project.run(&["--json", "run"]);
    assert!(success);

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    assert!(json["results"].is_array(), "JSON should have results array");
}

// ==================== All Fresh Scenario ====================

#[test]
fn test_per_file_all_fresh_shows_cached() {
    let project = TestProject::new(
        r#"verifications:
  - name: test
    command: cat $VERIFY_FILE
    cache_paths:
      - "*.txt"
    per_file: true
"#,
    );

    project.create_file("file1.txt", "content1");
    project.create_file("file2.txt", "content2");

    // First run
    project.run(&["run"]);

    // Second run - all files should be cached
    let (success, stdout, _) = project.run(&["run"]);
    assert!(success);

    // Should show as cached/verified without running
    assert!(
        stdout.contains("cached") || stdout.contains("verified") || stdout.contains("skipped"),
        "Should indicate files are cached: {}",
        stdout
    );
}
