use anyhow::{Context, Result};
use blake3::Hasher;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Pattern for extracting a metadata value from command output
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MetadataPattern {
    /// Pattern with replacement - [pattern, replacement]
    WithReplacement(String, String),
    /// Simple pattern - extracts first capture group
    Simple(String),
}

/// Root configuration structure parsed from verify.yaml
#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub verifications: Vec<VerificationItem>,
}

/// Either a verification check or a subproject reference
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum VerificationItem {
    /// A subproject reference (has path, no command)
    Subproject(Subproject),
    /// A regular verification check (has command, no path)
    Verification(Verification),
}

impl VerificationItem {
    pub fn name(&self) -> &str {
        match self {
            VerificationItem::Verification(v) => &v.name,
            VerificationItem::Subproject(s) => &s.name,
        }
    }
}

/// A reference to a subproject with its own verify.yaml
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Subproject {
    /// Unique identifier for this subproject
    pub name: String,

    /// Path to directory containing verify.yaml (relative to current config)
    pub path: PathBuf,
}

/// A single verification check definition
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Verification {
    /// Unique identifier for this check
    pub name: String,

    /// Command to execute (shell command)
    pub command: String,

    /// Glob patterns for files that affect this check's cache validity
    /// If empty or not specified, the check always runs (no verify-level caching)
    #[serde(default)]
    pub cache_paths: Vec<String>,

    /// Names of checks that must run before this one
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Optional: timeout in seconds (defaults to no timeout)
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Metadata extraction patterns
    /// Keys are metadata field names, values are regex patterns or [pattern, replacement] arrays
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, MetadataPattern>,

    /// Run command once per stale file (sets VERIFY_FILE env var)
    #[serde(default)]
    pub per_file: bool,
}

impl Verification {
    /// Compute a deterministic hash of this check's configuration.
    /// Used to detect when the check definition changes in verify.yaml.
    pub fn config_hash(&self) -> String {
        let mut hasher = Hasher::new();

        // Hash command
        hasher.update(b"command:");
        hasher.update(self.command.as_bytes());
        hasher.update(b"\n");

        // Hash cache_paths (sorted for determinism)
        hasher.update(b"cache_paths:");
        let mut sorted_paths = self.cache_paths.clone();
        sorted_paths.sort();
        for path in &sorted_paths {
            hasher.update(path.as_bytes());
            hasher.update(b",");
        }
        hasher.update(b"\n");

        // Hash timeout
        hasher.update(b"timeout:");
        if let Some(timeout) = self.timeout_secs {
            hasher.update(timeout.to_string().as_bytes());
        }
        hasher.update(b"\n");

        // Hash per_file flag
        hasher.update(b"per_file:");
        hasher.update(if self.per_file { b"true" } else { b"false" });
        hasher.update(b"\n");

        // Hash metadata patterns (sorted keys for determinism)
        hasher.update(b"metadata:");
        let mut sorted_keys: Vec<_> = self.metadata.keys().collect();
        sorted_keys.sort();
        for key in sorted_keys {
            hasher.update(key.as_bytes());
            hasher.update(b"=");
            match &self.metadata[key] {
                MetadataPattern::Simple(pattern) => {
                    hasher.update(pattern.as_bytes());
                }
                MetadataPattern::WithReplacement(pattern, replacement) => {
                    hasher.update(pattern.as_bytes());
                    hasher.update(b"|");
                    hasher.update(replacement.as_bytes());
                }
            }
            hasher.update(b",");
        }

        hasher.finalize().to_hex().to_string()
    }
}

impl Config {
    /// Load configuration from a YAML file
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with_base(path, path.parent().unwrap_or(Path::new(".")))
    }

    /// Load configuration with a specific base path for resolving subproject paths
    pub fn load_with_base(path: &Path, base_path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: Config = serde_yml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        config.validate(base_path)?;
        Ok(config)
    }

    /// Validate the configuration
    fn validate(&self, base_path: &Path) -> Result<()> {
        let mut names = HashSet::new();

        // Check for duplicate names
        for item in &self.verifications {
            let name = item.name();
            if !names.insert(name.to_string()) {
                anyhow::bail!("Duplicate verification name: {}", name);
            }
        }

        // Check that all dependencies exist (can depend on verifications OR subprojects)
        for item in &self.verifications {
            if let VerificationItem::Verification(v) = item {
                for dep in &v.depends_on {
                    if !names.contains(dep) {
                        anyhow::bail!(
                            "Verification '{}' depends on unknown check: {}",
                            v.name,
                            dep
                        );
                    }
                }

                // Check for self-dependencies
                if v.depends_on.contains(&v.name) {
                    anyhow::bail!("Verification '{}' cannot depend on itself", v.name);
                }
            }
        }

        // Validate subproject paths exist
        for item in &self.verifications {
            if let VerificationItem::Subproject(s) = item {
                let subproject_dir = base_path.join(&s.path);
                let subproject_config = subproject_dir.join("verify.yaml");
                if !subproject_config.exists() {
                    anyhow::bail!(
                        "Subproject '{}' config not found: {}",
                        s.name,
                        subproject_config.display()
                    );
                }
            }
        }

        Ok(())
    }

    /// Get a verification by name (returns None for subprojects)
    pub fn get(&self, name: &str) -> Option<&Verification> {
        self.verifications.iter().find_map(|item| match item {
            VerificationItem::Verification(v) if v.name == name => Some(v),
            _ => None,
        })
    }

    /// Get all verifications (excluding subprojects)
    pub fn verifications_only(&self) -> Vec<&Verification> {
        self.verifications
            .iter()
            .filter_map(|item| match item {
                VerificationItem::Verification(v) => Some(v),
                VerificationItem::Subproject(_) => None,
            })
            .collect()
    }

    /// Get all subprojects
    pub fn subprojects(&self) -> Vec<&Subproject> {
        self.verifications
            .iter()
            .filter_map(|item| match item {
                VerificationItem::Subproject(s) => Some(s),
                VerificationItem::Verification(_) => None,
            })
            .collect()
    }

    /// Get a subproject by name
    pub fn get_subproject(&self, name: &str) -> Option<&Subproject> {
        self.verifications.iter().find_map(|item| match item {
            VerificationItem::Subproject(s) if s.name == name => Some(s),
            _ => None,
        })
    }

    /// Check if a name refers to a subproject
    #[allow(dead_code)]
    pub fn is_subproject(&self, name: &str) -> bool {
        self.get_subproject(name).is_some()
    }
}

/// Generate an example configuration file
pub fn generate_example_config() -> String {
    r#"# verify configuration file
# Run `verify` to execute all stale checks, or `verify status` to see check states

verifications:
  - name: build
    command: npm run build
    cache_paths:
      - "src/**/*.ts"
      - "src/**/*.tsx"
      - "package.json"
      - "tsconfig.json"

  - name: typecheck
    command: npm run typecheck
    cache_paths:
      - "src/**/*.ts"
      - "src/**/*.tsx"
      - "tsconfig.json"

  - name: lint
    command: npm run lint
    cache_paths:
      - "src/**/*.ts"
      - "src/**/*.tsx"
      - ".eslintrc*"

  - name: test
    command: npm test
    depends_on: [build]
    cache_paths:
      - "src/**/*.ts"
      - "src/**/*.tsx"
      - "tests/**/*.ts"
      - "jest.config.*"
"#
    .to_string()
}

/// Initialize a new config file
pub fn init_config(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        anyhow::bail!(
            "Config file already exists: {}. Use --force to overwrite.",
            path.display()
        );
    }

    let content = generate_example_config();
    fs::write(path, content)
        .with_context(|| format!("Failed to write config file: {}", path.display()))?;

    // Add .verify/ to .gitignore if not already present
    let gitignore_path = path.parent().unwrap_or(Path::new(".")).join(".gitignore");
    let cache_pattern = "**/.verify/";

    let should_append = if gitignore_path.exists() {
        let gitignore_content = fs::read_to_string(&gitignore_path)
            .with_context(|| format!("Failed to read .gitignore: {}", gitignore_path.display()))?;
        !gitignore_content.lines().any(|line| line.trim() == cache_pattern)
    } else {
        true
    };

    if should_append {
        use std::fs::OpenOptions;
        use std::io::Write;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
            .with_context(|| format!("Failed to open .gitignore: {}", gitignore_path.display()))?;

        // Add newline before if file exists and doesn't end with newline
        if gitignore_path.exists() {
            let content = fs::read_to_string(&gitignore_path).unwrap_or_default();
            if !content.is_empty() && !content.ends_with('\n') {
                writeln!(file)?;
            }
        }

        writeln!(file, "{}", cache_pattern)
            .with_context(|| "Failed to write to .gitignore")?;
    }

    // Add verify.lock merge strategy to .gitattributes
    let gitattributes_path = path.parent().unwrap_or(Path::new(".")).join(".gitattributes");
    let lock_pattern = "verify.lock merge=ours";

    let should_append_gitattributes = if gitattributes_path.exists() {
        let gitattributes_content = fs::read_to_string(&gitattributes_path)
            .with_context(|| format!("Failed to read .gitattributes: {}", gitattributes_path.display()))?;
        !gitattributes_content.lines().any(|line| line.trim() == lock_pattern)
    } else {
        true
    };

    if should_append_gitattributes {
        use std::fs::OpenOptions;
        use std::io::Write;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitattributes_path)
            .with_context(|| format!("Failed to open .gitattributes: {}", gitattributes_path.display()))?;

        // Add newline before if file exists and doesn't end with newline
        if gitattributes_path.exists() {
            let content = fs::read_to_string(&gitattributes_path).unwrap_or_default();
            if !content.is_empty() && !content.ends_with('\n') {
                writeln!(file)?;
            }
        }

        writeln!(file, "{}", lock_pattern)
            .with_context(|| "Failed to write to .gitattributes")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let yaml = r#"
verifications:
  - name: test
    command: npm test
    cache_paths:
      - "src/**/*.ts"
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        assert_eq!(config.verifications.len(), 1);
        assert_eq!(config.verifications[0].name(), "test");
    }

    #[test]
    fn test_parse_subproject() {
        let yaml = r#"
verifications:
  - name: frontend
    path: ./packages/frontend
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        assert_eq!(config.verifications.len(), 1);
        match &config.verifications[0] {
            VerificationItem::Subproject(s) => {
                assert_eq!(s.name, "frontend");
                assert_eq!(s.path, PathBuf::from("./packages/frontend"));
            }
            _ => panic!("Expected Subproject"),
        }
    }

    #[test]
    fn test_duplicate_names() {
        let yaml = r#"
verifications:
  - name: test
    command: npm test
    cache_paths: []
  - name: test
    command: npm test
    cache_paths: []
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        assert!(config.validate(Path::new(".")).is_err());
    }

    #[test]
    fn test_unknown_dependency() {
        let yaml = r#"
verifications:
  - name: test
    command: npm test
    cache_paths: []
    depends_on: [nonexistent]
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        assert!(config.validate(Path::new(".")).is_err());
    }

    #[test]
    fn test_mixed_verifications_and_subprojects() {
        let yaml = r#"
verifications:
  - name: build
    command: npm run build
    cache_paths: ["src/**/*.ts"]
  - name: frontend
    path: ./packages/frontend
  - name: lint
    command: npm run lint
    cache_paths: ["src/**/*.ts"]
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        assert_eq!(config.verifications.len(), 3);
        assert_eq!(config.verifications_only().len(), 2);
        assert_eq!(config.subprojects().len(), 1);
    }

    #[test]
    fn test_verification_depends_on_subproject() {
        // Verifications can depend on subprojects - this should parse without error
        let yaml = r#"
verifications:
  - name: frontend
    path: ./packages/frontend
  - name: backend
    path: ./packages/backend
  - name: integration
    command: npm run integration
    depends_on: [frontend, backend]
    cache_paths: ["tests/**/*.ts"]
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        assert_eq!(config.verifications.len(), 3);
        assert_eq!(config.subprojects().len(), 2);

        // The verification should have depends_on containing subproject names
        let integration = config.get("integration").unwrap();
        assert!(integration.depends_on.contains(&"frontend".to_string()));
        assert!(integration.depends_on.contains(&"backend".to_string()));
    }

    // ==================== Self-dependency tests ====================

    #[test]
    fn test_self_dependency_rejected() {
        // A check cannot depend on itself
        let yaml = r#"
verifications:
  - name: build
    command: npm run build
    cache_paths: []
    depends_on: [build]
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        let result = config.validate(Path::new("."));
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(err.contains("cannot depend on itself"));
        assert!(err.contains("build"));
    }

    #[test]
    fn test_self_dependency_among_valid_deps() {
        // Self-dependency hidden among valid dependencies should still be rejected
        let yaml = r#"
verifications:
  - name: lint
    command: npm run lint
    cache_paths: []
  - name: build
    command: npm run build
    cache_paths: []
    depends_on: [lint, build]
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        let result = config.validate(Path::new("."));
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(err.contains("cannot depend on itself"));
    }

    // ==================== Empty config tests ====================

    #[test]
    fn test_empty_verifications() {
        let yaml = r#"
verifications: []
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        assert!(config.validate(Path::new(".")).is_ok());
        assert!(config.verifications.is_empty());
    }

    // ==================== Config hash tests ====================

    #[test]
    fn test_config_hash_determinism() {
        let v1 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec!["src/**/*.ts".to_string()],
            depends_on: vec![],
            timeout_secs: Some(300),
            metadata: HashMap::new(),
            per_file: false,
        };

        let v2 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec!["src/**/*.ts".to_string()],
            depends_on: vec![],
            timeout_secs: Some(300),
            metadata: HashMap::new(),
            per_file: false,
        };

        assert_eq!(v1.config_hash(), v2.config_hash());
    }

    #[test]
    fn test_config_hash_changes_with_command() {
        let v1 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec![],
            depends_on: vec![],
            timeout_secs: None,
            metadata: HashMap::new(),
            per_file: false,
        };

        let v2 = Verification {
            name: "test".to_string(),
            command: "npm run test".to_string(), // different command
            cache_paths: vec![],
            depends_on: vec![],
            timeout_secs: None,
            metadata: HashMap::new(),
            per_file: false,
        };

        assert_ne!(v1.config_hash(), v2.config_hash());
    }

    #[test]
    fn test_config_hash_changes_with_cache_paths() {
        let v1 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec!["src/**/*.ts".to_string()],
            depends_on: vec![],
            timeout_secs: None,
            metadata: HashMap::new(),
            per_file: false,
        };

        let v2 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec!["src/**/*.js".to_string()], // different path
            depends_on: vec![],
            timeout_secs: None,
            metadata: HashMap::new(),
            per_file: false,
        };

        assert_ne!(v1.config_hash(), v2.config_hash());
    }

    #[test]
    fn test_config_hash_changes_with_timeout() {
        let v1 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec![],
            depends_on: vec![],
            timeout_secs: Some(300),
            metadata: HashMap::new(),
            per_file: false,
        };

        let v2 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec![],
            depends_on: vec![],
            timeout_secs: Some(600), // different timeout
            metadata: HashMap::new(),
            per_file: false,
        };

        assert_ne!(v1.config_hash(), v2.config_hash());
    }

    #[test]
    fn test_config_hash_changes_with_per_file() {
        let v1 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec![],
            depends_on: vec![],
            timeout_secs: None,
            metadata: HashMap::new(),
            per_file: false,
        };

        let v2 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec![],
            depends_on: vec![],
            timeout_secs: None,
            metadata: HashMap::new(),
            per_file: true, // different per_file setting
        };

        assert_ne!(v1.config_hash(), v2.config_hash());
    }

    #[test]
    fn test_config_hash_cache_paths_order_independent() {
        // Cache paths should be sorted, so order doesn't matter
        let v1 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec!["a.ts".to_string(), "b.ts".to_string(), "c.ts".to_string()],
            depends_on: vec![],
            timeout_secs: None,
            metadata: HashMap::new(),
            per_file: false,
        };

        let v2 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec!["c.ts".to_string(), "a.ts".to_string(), "b.ts".to_string()],
            depends_on: vec![],
            timeout_secs: None,
            metadata: HashMap::new(),
            per_file: false,
        };

        assert_eq!(v1.config_hash(), v2.config_hash());
    }

    #[test]
    fn test_config_hash_with_metadata() {
        use crate::config::MetadataPattern;

        let mut metadata1 = HashMap::new();
        metadata1.insert("coverage".to_string(), MetadataPattern::Simple(r"(\d+)%".to_string()));

        let v1 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec![],
            depends_on: vec![],
            timeout_secs: None,
            metadata: metadata1,
            per_file: false,
        };

        let v2 = Verification {
            name: "test".to_string(),
            command: "npm test".to_string(),
            cache_paths: vec![],
            depends_on: vec![],
            timeout_secs: None,
            metadata: HashMap::new(), // no metadata
            per_file: false,
        };

        assert_ne!(v1.config_hash(), v2.config_hash());
    }

    // ==================== Invalid YAML tests ====================

    #[test]
    fn test_invalid_yaml_syntax() {
        let yaml = r#"
verifications:
  - name: test
    command: npm test
    cache_paths: [invalid yaml here
"#;
        let result: Result<Config, _> = serde_yml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_command_parses_as_subproject() {
        // Without a command, serde's untagged enum parses this as a Subproject
        // (since Subproject only requires name + path, and cache_paths is ignored)
        // This is expected behavior due to serde's untagged enum matching
        let yaml = r#"
verifications:
  - name: test
    cache_paths: []
"#;
        let result: Result<Config, _> = serde_yml::from_str(yaml);
        // Parsing fails because without command or path, neither variant matches
        assert!(result.is_err());
    }

    // ==================== Special characters tests ====================

    #[test]
    fn test_special_characters_in_name() {
        let yaml = r#"
verifications:
  - name: "test-with-dashes"
    command: npm test
    cache_paths: []
  - name: "test_with_underscores"
    command: npm test
    cache_paths: []
  - name: "test.with.dots"
    command: npm test
    cache_paths: []
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        assert!(config.validate(Path::new(".")).is_ok());
        assert_eq!(config.verifications.len(), 3);
    }

    #[test]
    fn test_unicode_in_command() {
        let yaml = r#"
verifications:
  - name: test
    command: echo "Hello 世界 🎉"
    cache_paths: []
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        assert!(config.validate(Path::new(".")).is_ok());
        let test = config.get("test").unwrap();
        assert!(test.command.contains("世界"));
        assert!(test.command.contains("🎉"));
    }

    // ==================== Getter method tests ====================

    #[test]
    fn test_get_nonexistent_check() {
        let yaml = r#"
verifications:
  - name: build
    command: npm run build
    cache_paths: []
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        assert!(config.get("nonexistent").is_none());
    }

    #[test]
    fn test_get_subproject_via_get_returns_none() {
        // get() only returns Verifications, not Subprojects
        let yaml = r#"
verifications:
  - name: frontend
    path: ./packages/frontend
"#;
        let config: Config = serde_yml::from_str(yaml).unwrap();
        assert!(config.get("frontend").is_none()); // Returns None for subproject
        assert!(config.get_subproject("frontend").is_some()); // But get_subproject works
    }
}
