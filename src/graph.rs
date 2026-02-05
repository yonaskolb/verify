use crate::config::{Config, Verification};
use anyhow::Result;
use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;

/// Represents the dependency graph of verifications
pub struct DependencyGraph {
    graph: DiGraph<String, ()>,
    #[allow(dead_code)]
    name_to_node: HashMap<String, NodeIndex>,
}

impl DependencyGraph {
    /// Build a dependency graph from configuration (verifications only, not subprojects)
    pub fn from_config(config: &Config) -> Result<Self> {
        Self::from_verifications(&config.verifications_only())
    }

    /// Build a dependency graph from a list of verifications
    pub fn from_verifications(verifications: &[&Verification]) -> Result<Self> {
        let mut graph = DiGraph::new();
        let mut name_to_node = HashMap::new();

        // Add all nodes
        for v in verifications {
            let node = graph.add_node(v.name.clone());
            name_to_node.insert(v.name.clone(), node);
        }

        // Add edges (dependency -> dependent)
        // Skip dependencies that are subprojects (not in this graph)
        for v in verifications {
            let dependent_node = name_to_node[&v.name];
            for dep_name in &v.depends_on {
                // Only add edge if dependency is a verification (in the graph)
                // Subproject dependencies are handled separately in the runner
                if let Some(&dep_node) = name_to_node.get(dep_name) {
                    graph.add_edge(dep_node, dependent_node, ());
                }
            }
        }

        let result = Self {
            graph,
            name_to_node,
        };

        // Validate no cycles
        result.validate_no_cycles()?;

        Ok(result)
    }

    /// Check for circular dependencies
    fn validate_no_cycles(&self) -> Result<()> {
        toposort(&self.graph, None).map_err(|cycle| {
            let node_name = &self.graph[cycle.node_id()];
            anyhow::anyhow!("Circular dependency detected involving: {}", node_name)
        })?;
        Ok(())
    }

    /// Get execution order respecting dependencies
    /// Returns groups of checks that can be run in parallel
    pub fn execution_waves(&self) -> Vec<Vec<String>> {
        let mut waves = Vec::new();
        let mut completed: HashMap<NodeIndex, bool> = HashMap::new();

        // Initialize all nodes as not completed
        for node in self.graph.node_indices() {
            completed.insert(node, false);
        }

        loop {
            // Find all nodes whose dependencies are satisfied
            let mut wave = Vec::new();

            for node in self.graph.node_indices() {
                if completed[&node] {
                    continue;
                }

                // Check if all dependencies are completed
                let deps_satisfied = self
                    .graph
                    .neighbors_directed(node, petgraph::Direction::Incoming)
                    .all(|dep| completed[&dep]);

                if deps_satisfied {
                    wave.push(node);
                }
            }

            if wave.is_empty() {
                break;
            }

            // Mark this wave as completed and collect names
            let wave_names: Vec<String> = wave
                .iter()
                .map(|node| {
                    completed.insert(*node, true);
                    self.graph[*node].clone()
                })
                .collect();

            waves.push(wave_names);
        }

        waves
    }

    /// Get direct dependencies for a check
    #[allow(dead_code)]
    pub fn dependencies(&self, name: &str) -> Vec<String> {
        if let Some(&node) = self.name_to_node.get(name) {
            self.graph
                .neighbors_directed(node, petgraph::Direction::Incoming)
                .map(|n| self.graph[n].clone())
                .collect()
        } else {
            vec![]
        }
    }

    /// Get all transitive dependencies for a check (including the check itself)
    #[allow(dead_code)]
    pub fn transitive_dependencies(&self, name: &str) -> Vec<String> {
        let mut result = vec![name.to_string()];

        if let Some(&node) = self.name_to_node.get(name) {
            let mut visited = HashMap::new();
            self.collect_deps(node, &mut visited);

            for (dep_node, _) in visited {
                if self.graph[dep_node] != name {
                    result.push(self.graph[dep_node].clone());
                }
            }
        }

        result
    }

    #[allow(dead_code)]
    fn collect_deps(&self, node: NodeIndex, visited: &mut HashMap<NodeIndex, bool>) {
        if visited.contains_key(&node) {
            return;
        }
        visited.insert(node, true);

        for dep in self
            .graph
            .neighbors_directed(node, petgraph::Direction::Incoming)
        {
            self.collect_deps(dep, visited);
        }
    }

    /// Get checks that depend on the given check (dependents)
    #[allow(dead_code)]
    pub fn dependents(&self, name: &str) -> Vec<String> {
        if let Some(&node) = self.name_to_node.get(name) {
            self.graph
                .neighbors_directed(node, petgraph::Direction::Outgoing)
                .map(|n| self.graph[n].clone())
                .collect()
        } else {
            vec![]
        }
    }

    /// Filter checks to run, including necessary dependencies
    #[allow(dead_code)]
    pub fn checks_to_run<'a>(
        &self,
        config: &'a Config,
        requested: &[String],
    ) -> Vec<&'a Verification> {
        let verifications = config.verifications_only();

        if requested.is_empty() {
            // Run all checks
            return verifications;
        }

        // Collect all checks including dependencies
        let mut to_run = std::collections::HashSet::new();
        for name in requested {
            for dep in self.transitive_dependencies(name) {
                to_run.insert(dep);
            }
        }

        // Return in config order (respects topological ordering within waves)
        verifications
            .into_iter()
            .filter(|v| to_run.contains(&v.name))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Verification, VerificationItem};

    fn make_config(verifications: Vec<(&str, Vec<&str>)>) -> Config {
        Config {
            verifications: verifications
                .into_iter()
                .map(|(name, deps)| {
                    VerificationItem::Verification(Verification {
                        name: name.to_string(),
                        command: "echo test".to_string(),
                        cache_paths: vec![],
                        depends_on: deps.into_iter().map(String::from).collect(),
                        timeout_secs: None,
                        metadata: std::collections::HashMap::new(),
                        per_file: false,
                    })
                })
                .collect(),
        }
    }

    // ==================== execution_waves tests ====================

    #[test]
    fn test_no_dependencies() {
        let config = make_config(vec![("a", vec![]), ("b", vec![]), ("c", vec![])]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let waves = graph.execution_waves();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].len(), 3);
    }

    #[test]
    fn test_linear_dependencies() {
        let config = make_config(vec![("a", vec![]), ("b", vec!["a"]), ("c", vec!["b"])]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let waves = graph.execution_waves();
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0], vec!["a"]);
        assert_eq!(waves[1], vec!["b"]);
        assert_eq!(waves[2], vec!["c"]);
    }

    #[test]
    fn test_diamond_dependency() {
        // a -> b, a -> c, b -> d, c -> d
        let config = make_config(vec![
            ("a", vec![]),
            ("b", vec!["a"]),
            ("c", vec!["a"]),
            ("d", vec!["b", "c"]),
        ]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let waves = graph.execution_waves();
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0], vec!["a"]);
        assert!(waves[1].contains(&"b".to_string()));
        assert!(waves[1].contains(&"c".to_string()));
        assert_eq!(waves[2], vec!["d"]);
    }

    #[test]
    fn test_cycle_detection() {
        let config = make_config(vec![("a", vec!["b"]), ("b", vec!["a"])]);
        let result = DependencyGraph::from_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_transitive_dependencies() {
        let config = make_config(vec![("a", vec![]), ("b", vec!["a"]), ("c", vec!["b"])]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let deps = graph.transitive_dependencies("c");
        assert!(deps.contains(&"a".to_string()));
        assert!(deps.contains(&"b".to_string()));
        assert!(deps.contains(&"c".to_string()));
    }

    #[test]
    fn test_single_node() {
        let config = make_config(vec![("only", vec![])]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let waves = graph.execution_waves();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0], vec!["only"]);
    }

    #[test]
    fn test_empty_graph() {
        let config = make_config(vec![]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let waves = graph.execution_waves();
        assert!(waves.is_empty());
    }

    #[test]
    fn test_two_independent_chains() {
        // Two separate dependency chains that can run in parallel
        // Chain 1: a -> b -> c
        // Chain 2: x -> y -> z
        let config = make_config(vec![
            ("a", vec![]),
            ("b", vec!["a"]),
            ("c", vec!["b"]),
            ("x", vec![]),
            ("y", vec!["x"]),
            ("z", vec!["y"]),
        ]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let waves = graph.execution_waves();
        assert_eq!(waves.len(), 3);

        // Wave 1: a and x (both roots)
        assert_eq!(waves[0].len(), 2);
        assert!(waves[0].contains(&"a".to_string()));
        assert!(waves[0].contains(&"x".to_string()));

        // Wave 2: b and y
        assert_eq!(waves[1].len(), 2);
        assert!(waves[1].contains(&"b".to_string()));
        assert!(waves[1].contains(&"y".to_string()));

        // Wave 3: c and z
        assert_eq!(waves[2].len(), 2);
        assert!(waves[2].contains(&"c".to_string()));
        assert!(waves[2].contains(&"z".to_string()));
    }

    #[test]
    fn test_wide_parallel_then_converge() {
        // Multiple independent checks converging to one final check
        // a, b, c, d all independent -> final depends on all
        let config = make_config(vec![
            ("a", vec![]),
            ("b", vec![]),
            ("c", vec![]),
            ("d", vec![]),
            ("final", vec!["a", "b", "c", "d"]),
        ]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let waves = graph.execution_waves();
        assert_eq!(waves.len(), 2);

        // Wave 1: all independent checks
        assert_eq!(waves[0].len(), 4);
        assert!(waves[0].contains(&"a".to_string()));
        assert!(waves[0].contains(&"b".to_string()));
        assert!(waves[0].contains(&"c".to_string()));
        assert!(waves[0].contains(&"d".to_string()));

        // Wave 2: final
        assert_eq!(waves[1].len(), 1);
        assert_eq!(waves[1][0], "final");
    }

    #[test]
    fn test_complex_dag() {
        // Complex DAG:
        //     a
        //    / \
        //   b   c
        //   |\ /|
        //   | X |
        //   |/ \|
        //   d   e
        //    \ /
        //     f
        let config = make_config(vec![
            ("a", vec![]),
            ("b", vec!["a"]),
            ("c", vec!["a"]),
            ("d", vec!["b", "c"]),
            ("e", vec!["b", "c"]),
            ("f", vec!["d", "e"]),
        ]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let waves = graph.execution_waves();
        assert_eq!(waves.len(), 4);

        assert_eq!(waves[0], vec!["a"]);

        assert_eq!(waves[1].len(), 2);
        assert!(waves[1].contains(&"b".to_string()));
        assert!(waves[1].contains(&"c".to_string()));

        assert_eq!(waves[2].len(), 2);
        assert!(waves[2].contains(&"d".to_string()));
        assert!(waves[2].contains(&"e".to_string()));

        assert_eq!(waves[3], vec!["f"]);
    }

    #[test]
    fn test_three_node_cycle() {
        let config = make_config(vec![("a", vec!["c"]), ("b", vec!["a"]), ("c", vec!["b"])]);
        let result = DependencyGraph::from_config(&config);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("Circular dependency"));
    }

    #[test]
    fn test_self_referencing_node() {
        // A node depending on itself - should be caught as a cycle
        let config = make_config(vec![("a", vec!["a"])]);
        let result = DependencyGraph::from_config(&config);
        assert!(result.is_err());
    }

    // ==================== dependencies/dependents tests ====================

    #[test]
    fn test_dependencies_direct() {
        let config = make_config(vec![("a", vec![]), ("b", vec![]), ("c", vec!["a", "b"])]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let deps = graph.dependencies("c");
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&"a".to_string()));
        assert!(deps.contains(&"b".to_string()));
    }

    #[test]
    fn test_dependencies_none() {
        let config = make_config(vec![("a", vec![]), ("b", vec![])]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let deps = graph.dependencies("a");
        assert!(deps.is_empty());
    }

    #[test]
    fn test_dependencies_unknown_node() {
        let config = make_config(vec![("a", vec![])]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let deps = graph.dependencies("nonexistent");
        assert!(deps.is_empty());
    }

    #[test]
    fn test_dependents_direct() {
        let config = make_config(vec![("a", vec![]), ("b", vec!["a"]), ("c", vec!["a"])]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let dependents = graph.dependents("a");
        assert_eq!(dependents.len(), 2);
        assert!(dependents.contains(&"b".to_string()));
        assert!(dependents.contains(&"c".to_string()));
    }

    #[test]
    fn test_dependents_none() {
        let config = make_config(vec![("a", vec![]), ("b", vec!["a"])]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        // b has no dependents (it's a leaf)
        let dependents = graph.dependents("b");
        assert!(dependents.is_empty());
    }

    #[test]
    fn test_transitive_dependencies_single_node() {
        let config = make_config(vec![("a", vec![])]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let deps = graph.transitive_dependencies("a");
        assert_eq!(deps.len(), 1);
        assert!(deps.contains(&"a".to_string()));
    }

    #[test]
    fn test_transitive_dependencies_deep_chain() {
        let config = make_config(vec![
            ("a", vec![]),
            ("b", vec!["a"]),
            ("c", vec!["b"]),
            ("d", vec!["c"]),
            ("e", vec!["d"]),
        ]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let deps = graph.transitive_dependencies("e");
        assert_eq!(deps.len(), 5);
        assert!(deps.contains(&"a".to_string()));
        assert!(deps.contains(&"b".to_string()));
        assert!(deps.contains(&"c".to_string()));
        assert!(deps.contains(&"d".to_string()));
        assert!(deps.contains(&"e".to_string()));
    }

    #[test]
    fn test_transitive_dependencies_unknown_node() {
        let config = make_config(vec![("a", vec![])]);
        let graph = DependencyGraph::from_config(&config).unwrap();

        let deps = graph.transitive_dependencies("nonexistent");
        // Should return just the requested name even if not found
        assert_eq!(deps.len(), 1);
        assert!(deps.contains(&"nonexistent".to_string()));
    }
}
