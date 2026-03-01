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
        stdout_a.contains("unverified") || stdout_a.contains("changed"),
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

// ==================== Subproject Status Propagation Tests ====================

#[test]
fn test_status_verification_verified_when_subproject_verified() {
    // A verification depending on a subproject should be "verified"
    // when all the subproject's checks are verified
    let project = TestProject::new(
        r#"verifications:
  - name: backend
    path: packages/backend
  - name: integration_test
    command: echo "integration"
    depends_on: [backend]
    cache_paths:
      - "*.txt"
"#,
    );

    project.add_subproject(
        "packages/backend",
        r#"verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.rs"
"#,
    );

    project.create_file("test.txt", "root content");
    project.create_subproject_file("packages/backend", "lib.rs", "fn main() {}");

    // Run everything first so caches are populated
    let (success, _, _) = project.run(&["run"]);
    assert!(success);

    // Now check status — integration_test should be verified (not "depends on: backend")
    let (_, stdout, _) = project.run(&["status"]);

    assert!(
        stdout.contains("integration_test") && stdout.contains("verified"),
        "integration_test should show as verified: {}",
        stdout
    );
    assert!(
        !stdout.contains("depends on: backend"),
        "integration_test should NOT show dependency unverified: {}",
        stdout
    );
}

#[test]
fn test_status_verification_unverified_when_subproject_unverified() {
    // A verification depending on a subproject should be "unverified"
    // when the subproject has checks that haven't been run yet
    let project = TestProject::new(
        r#"verifications:
  - name: backend
    path: packages/backend
  - name: integration_test
    command: echo "integration"
    depends_on: [backend]
    cache_paths:
      - "*.txt"
"#,
    );

    project.add_subproject(
        "packages/backend",
        r#"verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.rs"
"#,
    );

    project.create_file("test.txt", "root content");
    project.create_subproject_file("packages/backend", "lib.rs", "fn main() {}");

    // Don't run anything — subproject checks are never-run
    let (_, stdout, _) = project.run(&["status"]);

    assert!(
        stdout.contains("integration_test") && stdout.contains("unverified"),
        "integration_test should show as unverified: {}",
        stdout
    );
}

#[test]
fn test_status_aggregate_verified_when_subprojects_verified() {
    // An aggregate check (no command) depending on subprojects should be "verified"
    // when all subproject checks are verified
    let project = TestProject::new(
        r#"verifications:
  - name: frontend
    path: packages/frontend
  - name: backend
    path: packages/backend
  - name: all
    depends_on: [frontend, backend]
"#,
    );

    project.add_subproject(
        "packages/frontend",
        r#"verifications:
  - name: build
    command: echo "frontend build"
    cache_paths:
      - "*.js"
"#,
    );

    project.add_subproject(
        "packages/backend",
        r#"verifications:
  - name: build
    command: echo "backend build"
    cache_paths:
      - "*.rs"
"#,
    );

    project.create_subproject_file("packages/frontend", "app.js", "console.log('hi')");
    project.create_subproject_file("packages/backend", "main.rs", "fn main() {}");

    // Run everything
    let (success, _, _) = project.run(&["run"]);
    assert!(success);

    // Status: aggregate "all" should be verified
    let (_, stdout, _) = project.run(&["status"]);

    // Find the "all" line - should say verified, not unverified
    let all_line = stdout.lines().find(|l| l.contains("all") && !l.contains("all-")).unwrap_or("");
    assert!(
        all_line.contains("verified") && !all_line.contains("unverified"),
        "Aggregate 'all' should be verified: '{}'.\nFull output:\n{}",
        all_line,
        stdout
    );
}

#[test]
fn test_status_aggregate_unverified_when_subproject_unverified() {
    // An aggregate check (no command) depending on subprojects should be "unverified"
    // when any subproject has unverified checks
    let project = TestProject::new(
        r#"verifications:
  - name: frontend
    path: packages/frontend
  - name: backend
    path: packages/backend
  - name: all
    depends_on: [frontend, backend]
"#,
    );

    project.add_subproject(
        "packages/frontend",
        r#"verifications:
  - name: build
    command: echo "frontend build"
    cache_paths:
      - "*.js"
"#,
    );

    project.add_subproject(
        "packages/backend",
        r#"verifications:
  - name: build
    command: echo "backend build"
    cache_paths:
      - "*.rs"
"#,
    );

    project.create_subproject_file("packages/frontend", "app.js", "console.log('hi')");
    project.create_subproject_file("packages/backend", "main.rs", "fn main() {}");

    // Don't run — subproject checks never run
    let (_, stdout, _) = project.run(&["status"]);

    let all_line = stdout.lines().find(|l| l.contains("all") && !l.contains("all-")).unwrap_or("");
    assert!(
        all_line.contains("unverified"),
        "Aggregate 'all' should be unverified when subprojects haven't run: '{}'.\nFull output:\n{}",
        all_line,
        stdout
    );
}

#[test]
fn test_status_subproject_stale_after_file_change_propagates() {
    // After modifying a file in a subproject, verifications depending on
    // that subproject should show as unverified
    let project = TestProject::new(
        r#"verifications:
  - name: lib
    path: packages/lib
  - name: integration
    command: echo "integration"
    depends_on: [lib]
    cache_paths:
      - "*.txt"
"#,
    );

    project.add_subproject(
        "packages/lib",
        r#"verifications:
  - name: build
    command: echo "build"
    cache_paths:
      - "*.rs"
"#,
    );

    project.create_file("test.txt", "content");
    project.create_subproject_file("packages/lib", "lib.rs", "fn lib() {}");

    // Run everything first
    let (success, _, _) = project.run(&["run"]);
    assert!(success);

    // Modify a subproject file
    project.create_subproject_file("packages/lib", "lib.rs", "fn lib_v2() {}");

    // Status: integration should now be unverified because lib is stale
    let (_, stdout, _) = project.run(&["status"]);

    assert!(
        stdout.contains("integration") && stdout.contains("unverified"),
        "integration should be unverified after subproject file change: {}",
        stdout
    );
}

// ==================== Nested Subproject Staleness Propagation Tests ====================

#[test]
fn test_nested_subproject_verified_status_propagates_to_parent() {
    // Bug: check_has_stale doesn't pre-populate subproject staleness into is_stale,
    // so a verification depending on a sub-subproject is always treated as stale.
    // This causes the entire subproject to appear stale to the parent, even when
    // all checks have been run and are verified.
    //
    // Structure:
    //   root: subproject "frontend" + verification "deploy" depends_on: [frontend]
    //   packages/frontend: subproject "components" + verification "build" depends_on: [components]
    //   packages/frontend/components: verification "compile"
    //
    // After running everything, "build" in the frontend config depends on "components"
    // (a subproject). check_has_stale is called on the frontend config but doesn't
    // insert "components" staleness into is_stale, so "build" is always treated as
    // stale, making "frontend" appear stale, making "deploy" unverified.
    let project = TestProject::new(
        r#"verifications:
  - name: frontend
    path: packages/frontend
  - name: deploy
    command: echo "deploying"
    depends_on: [frontend]
    cache_paths:
      - "*.txt"
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
    cache_paths:
      - "*.ts"
"#,
    );

    project.add_subproject(
        "packages/frontend/components",
        r#"verifications:
  - name: compile
    command: echo "compiling components"
    cache_paths:
      - "*.ts"
"#,
    );

    project.create_file("root.txt", "root content");
    project.create_subproject_file("packages/frontend", "app.ts", "export default {}");
    project.create_subproject_file("packages/frontend/components", "button.ts", "export {}");

    // Run everything - should succeed
    let (success, stdout, stderr) = project.run(&["run"]);
    assert!(
        success,
        "Run should succeed. Stdout: {}\nStderr: {}",
        stdout, stderr
    );

    // Now check status — everything was just run, so all should be verified
    let (_, stdout, _) = project.run(&["status"]);

    // The deploy verification should be verified (not "depends on: frontend")
    assert!(
        !stdout.contains("depends on"),
        "No verification should show 'depends on' as unverified reason after a clean run.\nFull output:\n{}",
        stdout
    );

    // Specifically, deploy should be verified
    let deploy_line = stdout
        .lines()
        .find(|l| l.contains("deploy"))
        .unwrap_or("");
    assert!(
        deploy_line.contains("verified") && !deploy_line.contains("unverified"),
        "deploy should be verified after running everything: '{}'\nFull output:\n{}",
        deploy_line,
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
