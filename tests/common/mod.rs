/// Shared test fixtures and helpers for integration tests
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// Helper to get the path to the verify binary
pub fn verify_binary() -> PathBuf {
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

/// Run verify command and return (success, stdout, stderr)
pub fn run_verify(project_dir: &Path, args: &[&str]) -> (bool, String, String) {
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

/// Run verify command and return exit code
pub fn run_verify_exit_code(project_dir: &Path, args: &[&str]) -> i32 {
    let binary = verify_binary();
    let status = Command::new(&binary)
        .args(args)
        .current_dir(project_dir)
        .status()
        .unwrap_or_else(|e| panic!("Failed to execute verify at {:?}: {}", binary, e));

    status.code().unwrap_or(-1)
}

/// Helper to create a test project directory with a verify.yaml config
pub fn setup_test_project(config_yaml: &str) -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    fs::write(temp_dir.path().join("verify.yaml"), config_yaml).expect("Failed to write config");

    temp_dir
}

/// Create a test project with subprojects
pub struct TestProject {
    pub root: TempDir,
}

impl TestProject {
    /// Create a new test project with the given root config
    pub fn new(root_config: &str) -> Self {
        let root = TempDir::new().expect("Failed to create temp directory");
        fs::write(root.path().join("verify.yaml"), root_config).expect("Failed to write config");
        Self { root }
    }

    /// Add a subproject at the given path with the given config
    pub fn add_subproject(&self, subpath: &str, config: &str) -> &Self {
        let subproject_dir = self.root.path().join(subpath);
        fs::create_dir_all(&subproject_dir).expect("Failed to create subproject dir");
        fs::write(subproject_dir.join("verify.yaml"), config).expect("Failed to write config");
        self
    }

    /// Create a file in the root project
    pub fn create_file(&self, path: &str, content: &str) -> &Self {
        let file_path = self.root.path().join(path);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).ok();
        }
        fs::write(&file_path, content).expect("Failed to write file");
        self
    }

    /// Create a file in a subproject
    pub fn create_subproject_file(&self, subpath: &str, file_path: &str, content: &str) -> &Self {
        let full_path = self.root.path().join(subpath).join(file_path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).ok();
        }
        fs::write(&full_path, content).expect("Failed to write file");
        self
    }

    /// Run verify in the root project
    pub fn run(&self, args: &[&str]) -> (bool, String, String) {
        run_verify(self.root.path(), args)
    }

    /// Run verify in a subproject
    pub fn run_in_subproject(&self, subpath: &str, args: &[&str]) -> (bool, String, String) {
        run_verify(&self.root.path().join(subpath), args)
    }

    /// Get exit code from verify run
    pub fn run_exit_code(&self, args: &[&str]) -> i32 {
        run_verify_exit_code(self.root.path(), args)
    }

    /// Read the verify.lock file as JSON
    pub fn read_lock(&self) -> Option<serde_json::Value> {
        let lock_path = self.root.path().join("verify.lock");
        if lock_path.exists() {
            let content = fs::read_to_string(&lock_path).ok()?;
            serde_json::from_str(&content).ok()
        } else {
            None
        }
    }

    /// Read a subproject's verify.lock file as JSON
    pub fn read_subproject_lock(&self, subpath: &str) -> Option<serde_json::Value> {
        let lock_path = self.root.path().join(subpath).join("verify.lock");
        if lock_path.exists() {
            let content = fs::read_to_string(&lock_path).ok()?;
            serde_json::from_str(&content).ok()
        } else {
            None
        }
    }

    /// Check if a file exists in the root project
    pub fn file_exists(&self, path: &str) -> bool {
        self.root.path().join(path).exists()
    }

    /// Read a file from the root project
    pub fn read_file(&self, path: &str) -> Option<String> {
        fs::read_to_string(self.root.path().join(path)).ok()
    }

    /// Get the root path
    pub fn path(&self) -> &Path {
        self.root.path()
    }
}
