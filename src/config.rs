use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Root configuration structure parsed from vfy.yaml
#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub verifications: Vec<Verification>,
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
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: Config = serde_yml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        config.validate()?;
        Ok(config)
    }

    /// Validate the configuration
    fn validate(&self) -> Result<()> {
        let mut names = HashSet::new();

        // Check for duplicate names
        for v in &self.verifications {
            if !names.insert(&v.name) {
                anyhow::bail!("Duplicate verification name: {}", v.name);
            }
        }

        // Check that all dependencies exist
        for v in &self.verifications {
            for dep in &v.depends_on {
                if !names.contains(dep) {
                    anyhow::bail!(
                        "Verification '{}' depends on unknown check: {}",
                        v.name,
                        dep
                    );
                }
            }
        }

        // Check for self-dependencies
        for v in &self.verifications {
            if v.depends_on.contains(&v.name) {
                anyhow::bail!("Verification '{}' cannot depend on itself", v.name);
            }
        }

        Ok(())
    }

    /// Get a verification by name
    pub fn get(&self, name: &str) -> Option<&Verification> {
        self.verifications.iter().find(|v| v.name == name)
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
        assert_eq!(config.verifications[0].name, "test");
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
        assert!(config.validate().is_err());
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
        assert!(config.validate().is_err());
    }
}
