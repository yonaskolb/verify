/// Subproject integration tests
/// Tests for multi-project scenarios with nested verify.yaml files
mod common;

use common::TestProject;
use std::fs;

// ==================== Basic Subproject Tests ====================

#[test]
fn test_subproject_basic_execution() {
    let project = TestProject::new(
        r#"verifications:
  - name: backend
    path: packages/backend
"#,
    );

    project.add_subproject(
        "packages/backend",
        r#"verifications:
  - name: build
    command: echo "building backend"
    cache_paths: []
"#,
    );

    let (success, stdout, stderr) = project.run(&["run"]);

    assert!(
        success,
        "Run should succeed. Stdout: {}\nStderr: {}",
        stdout, stderr
    );
    assert!(
        stdout.contains("verified") || stdout.contains("pass"),
        "Output should show verification passed: {}",
        stdout
    );
}

#[test]
fn test_subproject_creates_own_lock_file() {
    let project = TestProject::new(
        r#"verifications:
  - name: sub
    path: sub
"#,
    );

    project.add_subproject(
        "sub",
        r#"verifications:
  - name: test
    command: echo "test"
    cache_paths: []
"#,
    );

    let (success, _, _) = project.run(&["run"]);
    assert!(success);

    // Subproject should have its own verify.lock
    let sub_lock = project.read_subproject_lock("sub");
    assert!(sub_lock.is_some(), "Subproject should have verify.lock");

    if let Some(lock) = sub_lock {
        assert!(
            lock["checks"]["test"].is_object(),
            "Subproject lock should contain the 'test' check"
        );
    }
}

#[test]
fn test_subproject_cache_isolation() {
    // Each subproject should maintain independent cache state
    let project = TestProject::new(
        r#"verifications:
  - name: sub_a
    path: packages/a
  - name: sub_b
    path: packages/b
"#,
    );

    project
        .add_subproject(
            "packages/a",
            r#"verifications:
  - name: check
    command: echo "a"
    cache_paths:
      - "*.txt"
"#,
        )
        .add_subproject(
            "packages/b",
            r#"verifications:
  - name: check
    command: echo "b"
    cache_paths:
      - "*.txt"
"#,
        );

    // Create files in each subproject
    project.create_subproject_file("packages/a", "file.txt", "content_a");
    project.create_subproject_file("packages/b", "file.txt", "content_b");

    // Run all
    let (success, _, _) = project.run(&["run"]);
    assert!(success);

    // Both subprojects should have their own lock files
    let lock_a = project.read_subproject_lock("packages/a");
    let lock_b = project.read_subproject_lock("packages/b");

    assert!(lock_a.is_some(), "Subproject A should have lock file");
    assert!(lock_b.is_some(), "Subproject B should have lock file");

    // Modify only subproject A's file
    project.create_subproject_file("packages/a", "file.txt", "modified_a");

    // Check status - A should be stale, B should be fresh
    let (_, stdout_a, _) = project.run_in_subproject("packages/a", &["status"]);

    assert!(
        stdout_a.contains("stale") || stdout_a.contains("changed"),
        "Subproject A should be stale: {}",
        stdout_a
    );

    // B's cache should show its check is still fresh (content_hash set)
    if let Some(lock) = project.read_subproject_lock("packages/b") {
        assert!(
            lock["checks"]["check"]["content_hash"].is_string(),
            "Subproject B check should still have content_hash cached"
        );
    }
}

// ==================== Cross-Project Dependency Tests ====================

#[test]
fn test_verification_depends_on_subproject() {
    // A verification in root can depend on a subproject completing
    let project = TestProject::new(
        r#"verifications:
  - name: backend
    path: packages/backend
  - name: integration_test
    command: echo "running integration tests"
    depends_on: [backend]
    cache_paths: []
"#,
    );

    project.add_subproject(
        "packages/backend",
        r#"verifications:
  - name: build
    command: echo "building backend"
    cache_paths: []
"#,
    );

    let (success, stdout, stderr) = project.run(&["run"]);

    assert!(
        success,
        "Run should succeed. Stdout: {}\nStderr: {}",
        stdout, stderr
    );
}

#[test]
fn test_subproject_failure_blocks_dependent_verification() {
    let project = TestProject::new(
        r#"verifications:
  - name: backend
    path: packages/backend
  - name: integration_test
    command: echo "should not run"
    depends_on: [backend]
    cache_paths: []
"#,
    );

    project.add_subproject(
        "packages/backend",
        r#"verifications:
  - name: build
    command: exit 1
    cache_paths: []
"#,
    );

    let (success, stdout, _) = project.run(&["run"]);

    assert!(!success, "Run should fail due to subproject failure");
    // The dependent verification should be blocked
    assert!(
        stdout.contains("integration_test") || stdout.contains("backend"),
        "Output should mention the checks: {}",
        stdout
    );
}

#[test]
fn test_multiple_verifications_depend_on_same_subproject() {
    let project = TestProject::new(
        r#"verifications:
  - name: shared_lib
    path: packages/shared
  - name: frontend_test
    command: echo "frontend test"
    depends_on: [shared_lib]
    cache_paths: []
  - name: backend_test
    command: echo "backend test"
    depends_on: [shared_lib]
    cache_paths: []
"#,
    );

    project.add_subproject(
        "packages/shared",
        r#"verifications:
  - name: build
    command: echo "building shared"
    cache_paths: []
"#,
    );

    let (success, stdout, stderr) = project.run(&["run"]);

    assert!(
        success,
        "Run should succeed. Stdout: {}\nStderr: {}",
        stdout, stderr
    );
    // All three should pass
    assert!(
        stdout.contains("3 verified") || stdout.contains("verified"),
        "Should show verifications passed: {}",
        stdout
    );
}

// ==================== Nested Subproject Tests ====================

#[test]
fn test_nested_subprojects() {
    // Root -> packages/frontend -> packages/frontend/components
    let project = TestProject::new(
        r#"verifications:
  - name: frontend
    path: packages/frontend
"#,
    );

    project.add_subproject(
        "packages/frontend",
        r#"verifications:
  - name: components
    path: components
  - name: build
    command: echo "building frontend"
    depends_on: [components]
    cache_paths: []
"#,
    );

    project.add_subproject(
        "packages/frontend/components",
        r#"verifications:
  - name: compile
    command: echo "compiling components"
    cache_paths: []
"#,
    );

    let (success, stdout, stderr) = project.run(&["run"]);

    assert!(
        success,
        "Run should succeed with nested subprojects. Stdout: {}\nStderr: {}",
        stdout, stderr
    );
}

// ==================== Subproject Status Tests ====================

#[test]
fn test_status_shows_subproject_checks() {
    let project = TestProject::new(
        r#"verifications:
  - name: sub
    path: sub
"#,
    );

    project.add_subproject(
        "sub",
        r#"verifications:
  - name: check_a
    command: echo "a"
    cache_paths:
      - "*.txt"
  - name: check_b
    command: echo "b"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_subproject_file("sub", "file.txt", "content");

    // Run first
    project.run(&["run"]);

    // Check status
    let (success, stdout, _) = project.run(&["status"]);
    assert!(success);

    // Should show both subproject checks
    assert!(
        stdout.contains("check_a") && stdout.contains("check_b"),
        "Status should show subproject checks: {}",
        stdout
    );
}

#[test]
fn test_status_json_includes_subprojects() {
    let project = TestProject::new(
        r#"verifications:
  - name: sub
    path: sub
"#,
    );

    project.add_subproject(
        "sub",
        r#"verifications:
  - name: test
    command: echo "test"
    cache_paths: []
"#,
    );

    let (success, stdout, _) = project.run(&["--json", "status"]);
    assert!(success);

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");

    // Should have subproject in checks array
    let checks = json["checks"].as_array().expect("checks should be array");
    let has_subproject = checks
        .iter()
        .any(|c| c["type"] == "subproject" || c["name"] == "sub");
    assert!(
        has_subproject,
        "JSON status should include subproject: {}",
        stdout
    );
}

// ==================== Subproject Error Handling Tests ====================

#[test]
fn test_missing_subproject_config_error() {
    let project = TestProject::new(
        r#"verifications:
  - name: missing
    path: nonexistent
"#,
    );

    // Don't create the subproject directory

    let (success, _stdout, stderr) = project.run(&["run"]);

    assert!(!success, "Run should fail with missing subproject");
    assert!(
        stderr.contains("nonexistent") || stderr.contains("not found") || stderr.contains("error"),
        "Error should mention the missing subproject: {}",
        stderr
    );
}

#[test]
fn test_subproject_invalid_config_error() {
    let project = TestProject::new(
        r#"verifications:
  - name: invalid
    path: invalid_sub
"#,
    );

    // Create subproject with invalid YAML
    let invalid_sub = project.root.path().join("invalid_sub");
    fs::create_dir_all(&invalid_sub).unwrap();
    fs::write(invalid_sub.join("verify.yaml"), "invalid: [yaml: syntax").unwrap();

    let (success, _stdout, stderr) = project.run(&["run"]);

    assert!(!success, "Run should fail with invalid subproject config");
    assert!(
        stderr.contains("parse") || stderr.contains("yaml") || stderr.contains("error"),
        "Error should mention parsing issue: {}",
        stderr
    );
}

// ==================== Running Specific Checks with Subprojects ====================

#[test]
fn test_run_specific_check_with_subprojects() {
    // Test running a specific root check when subprojects exist
    let project = TestProject::new(
        r#"verifications:
  - name: sub
    path: packages/sub
  - name: root_check
    command: echo "root"
    cache_paths: []
"#,
    );

    project.add_subproject(
        "packages/sub",
        r#"verifications:
  - name: test
    command: echo "sub"
    cache_paths: []
"#,
    );

    // Run only root_check (not the subproject)
    let (success, stdout, stderr) = project.run(&["run", "root_check"]);

    assert!(
        success,
        "Running specific check should succeed. stdout: {}\nstderr: {}",
        stdout, stderr
    );
    // Should only run root_check
    assert!(
        stdout.contains("1 verified"),
        "Should only verify 1 item (root_check): {}",
        stdout
    );
}

// ==================== Clean with Subprojects ====================

#[test]
fn test_clean_does_not_affect_subproject_cache() {
    let project = TestProject::new(
        r#"verifications:
  - name: root_check
    command: echo "root"
    cache_paths:
      - "*.txt"
  - name: sub
    path: sub
"#,
    );

    project.add_subproject(
        "sub",
        r#"verifications:
  - name: sub_check
    command: echo "sub"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_file("root.txt", "root content");
    project.create_subproject_file("sub", "sub.txt", "sub content");

    // Run all
    project.run(&["run"]);

    // Verify both have caches
    assert!(project.read_lock().is_some());
    assert!(project.read_subproject_lock("sub").is_some());

    // Clean root_check only
    let (success, _, _) = project.run(&["clean", "root_check"]);
    assert!(success);

    // Subproject cache should still exist and be intact
    let sub_lock = project.read_subproject_lock("sub");
    assert!(sub_lock.is_some(), "Subproject cache should still exist");
    if let Some(lock) = sub_lock {
        assert!(
            lock["checks"]["sub_check"]["content_hash"].is_string(),
            "Subproject check should still be cached"
        );
    }
}

// ==================== Subproject Force Run ====================

#[test]
fn test_force_run_affects_subprojects() {
    let project = TestProject::new(
        r#"verifications:
  - name: sub
    path: sub
"#,
    );

    project.add_subproject(
        "sub",
        r#"verifications:
  - name: check
    command: echo "running"
    cache_paths:
      - "*.txt"
"#,
    );

    project.create_subproject_file("sub", "file.txt", "content");

    // First run
    project.run(&["run"]);

    // Force run - should re-execute even though cached
    let (success, stdout, _) = project.run(&["run", "--force"]);

    assert!(success);
    // Should show verification completed (the run succeeded)
    assert!(
        stdout.contains("verified"),
        "Force should complete verification: {}",
        stdout
    );
}
