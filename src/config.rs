use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Root configuration structure parsed from vfy.yaml
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

/// A reference to a subproject with its own vfy.yaml
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Subproject {
    /// Unique identifier for this subproject
    pub name: String,

    /// Path to directory containing vfy.yaml (relative to current config)
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
    pub cache_paths: Vec<String>,

    /// Names of checks that must run before this one
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Optional: timeout in seconds (defaults to no timeout)
    #[serde(default)]
    pub timeout_secs: Option<u64>,
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
                let subproject_config = subproject_dir.join("vfy.yaml");
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
    pub fn is_subproject(&self, name: &str) -> bool {
        self.get_subproject(name).is_some()
    }
}

/// Generate an example configuration file
pub fn generate_example_config() -> String {
    r#"# vfy configuration file
# Run `vfy` to execute all stale checks, or `vfy status` to see check states

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
}
