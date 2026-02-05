use crate::config::MetadataPattern;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// A metadata value extracted from command output
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MetadataValue {
    Integer(i64),
    Float(f64),
    String(String),
}

impl fmt::Display for MetadataValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetadataValue::Integer(i) => write!(f, "{}", i),
            MetadataValue::Float(v) => write!(f, "{:.1}", v),
            MetadataValue::String(s) => write!(f, "{}", s),
        }
    }
}

/// Extract metadata from command output using configured patterns
pub fn extract_metadata(
    output: &str,
    patterns: &HashMap<String, MetadataPattern>,
) -> BTreeMap<String, MetadataValue> {
    let mut result = BTreeMap::new();

    for (key, pattern) in patterns {
        if let Some(value) = apply_pattern(output, pattern) {
            result.insert(key.clone(), parse_value(&value));
        }
    }

    result
}

fn apply_pattern(output: &str, pattern: &MetadataPattern) -> Option<String> {
    match pattern {
        MetadataPattern::Simple(pat) => {
            let re = Regex::new(pat).ok()?;
            // Use last match since relevant output is typically at the end
            let caps = re.captures_iter(output).last()?;
            caps.get(1).map(|m| m.as_str().to_string())
        }
        MetadataPattern::WithReplacement(pat, repl) => {
            let re = Regex::new(pat).ok()?;
            // Use last match since relevant output is typically at the end
            let caps = re.captures_iter(output).last()?;
            // Expand $1, $2, etc. in replacement string
            let mut result = repl.clone();
            for (i, cap) in caps.iter().enumerate().skip(1) {
                if let Some(m) = cap {
                    result = result.replace(&format!("${}", i), m.as_str());
                }
            }
            Some(result)
        }
    }
}

fn parse_value(s: &str) -> MetadataValue {
    // Try integer first
    if let Ok(i) = s.parse::<i64>() {
        return MetadataValue::Integer(i);
    }
    // Try float
    if let Ok(f) = s.parse::<f64>() {
        return MetadataValue::Float(f);
    }
    // Default to string
    MetadataValue::String(s.to_string())
}

/// Compute delta between two numeric metadata values
pub fn compute_delta(current: &MetadataValue, prev: &MetadataValue) -> Option<f64> {
    match (current, prev) {
        (MetadataValue::Integer(c), MetadataValue::Integer(p)) => Some((*c - *p) as f64),
        (MetadataValue::Float(c), MetadataValue::Float(p)) => Some(*c - *p),
        (MetadataValue::Integer(c), MetadataValue::Float(p)) => Some(*c as f64 - *p),
        (MetadataValue::Float(c), MetadataValue::Integer(p)) => Some(*c - *p as f64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_pattern() {
        let mut patterns = HashMap::new();
        patterns.insert(
            "count".to_string(),
            MetadataPattern::Simple(r"Total: (\d+)".to_string()),
        );

        let output = "Processing...\nTotal: 42 items\nDone!";
        let metadata = extract_metadata(output, &patterns);

        assert_eq!(metadata.len(), 1);
        match metadata.get("count") {
            Some(MetadataValue::Integer(42)) => {}
            other => panic!("Expected Integer(42), got {:?}", other),
        }
    }

    #[test]
    fn test_float_extraction() {
        let mut patterns = HashMap::new();
        patterns.insert(
            "coverage".to_string(),
            MetadataPattern::Simple(r"Coverage: ([\d.]+)%".to_string()),
        );

        let output = "Coverage: 85.5%";
        let metadata = extract_metadata(output, &patterns);

        match metadata.get("coverage") {
            Some(MetadataValue::Float(f)) => assert!((f - 85.5).abs() < 0.001),
            other => panic!("Expected Float(85.5), got {:?}", other),
        }
    }

    #[test]
    fn test_replacement_pattern() {
        let mut patterns = HashMap::new();
        patterns.insert(
            "ratio".to_string(),
            MetadataPattern::WithReplacement(r"(\d+)/(\d+)".to_string(), "$1 of $2".to_string()),
        );

        let output = "Results: 5/10";
        let metadata = extract_metadata(output, &patterns);

        match metadata.get("ratio") {
            Some(MetadataValue::String(s)) => assert_eq!(s, "5 of 10"),
            other => panic!("Expected String(\"5 of 10\"), got {:?}", other),
        }
    }

    #[test]
    fn test_no_match() {
        let mut patterns = HashMap::new();
        patterns.insert(
            "count".to_string(),
            MetadataPattern::Simple(r"Total: (\d+)".to_string()),
        );

        let output = "No total here";
        let metadata = extract_metadata(output, &patterns);

        assert!(metadata.is_empty());
    }

    #[test]
    fn test_compute_delta_integers() {
        let current = MetadataValue::Integer(10);
        let prev = MetadataValue::Integer(7);
        assert_eq!(compute_delta(&current, &prev), Some(3.0));
    }

    #[test]
    fn test_compute_delta_floats() {
        let current = MetadataValue::Float(85.5);
        let prev = MetadataValue::Float(80.0);
        let delta = compute_delta(&current, &prev).unwrap();
        assert!((delta - 5.5).abs() < 0.001);
    }

    #[test]
    fn test_compute_delta_strings() {
        let current = MetadataValue::String("a".to_string());
        let prev = MetadataValue::String("b".to_string());
        assert_eq!(compute_delta(&current, &prev), None);
    }

    #[test]
    fn test_multiple_matches_uses_last() {
        let mut patterns = HashMap::new();
        patterns.insert(
            "count".to_string(),
            MetadataPattern::Simple(r"Total: (\d+)".to_string()),
        );

        // Output with multiple matches - should use the last one (99)
        let output = "Total: 10\nProcessing...\nTotal: 50\nMore work...\nTotal: 99";
        let metadata = extract_metadata(output, &patterns);

        match metadata.get("count") {
            Some(MetadataValue::Integer(99)) => {}
            other => panic!("Expected Integer(99) (last match), got {:?}", other),
        }
    }
}
