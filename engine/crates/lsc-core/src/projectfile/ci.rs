// SPDX-License-Identifier: Apache-2.0 OR MIT
//! `org.projectfile.ci` Intent DAG Resolution Engine.
//!
//! Conforms to normative specification <https://projectfile.org/spec/v1>
//! and conformance vectors under `spec/conformance/ci/*.yaml`.
//!
//! # Invariants
//! - Zero-I/O: operates purely on in-memory buffers and serde values.
//! - Strict cycle refusal via three-color depth-first search.
//! - Automatic sink vs explicit goal inference.
//! - Reachable needs-closure pruning.
//! - Cell fan-out isolation (source runs 1x, join runs 1x, cell runs Nx).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Matrix specification defining multidimensional build axes and exclusions.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CiMatrixDef {
    /// Named axes mapping to discrete value lists.
    pub axes: BTreeMap<String, Vec<String>>,
    /// Axis coordinate combinations to exclude from Cartesian product.
    pub exclude: Vec<BTreeMap<String, String>>,
}

/// Options or arguments bound to a dependency need.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CiNeedOption {
    /// Whether this dependency is active.
    pub enabled: bool,
    /// Command-line arguments passed to the tool invocation.
    pub args: Option<String>,
}

/// A node definition in the CI intent DAG.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CiNode {
    /// Dependencies (tools or other nodes).
    pub needs: BTreeMap<String, CiNeedOption>,
    /// Declared order of dependencies, preserving sequence order when `needs` is a YAML sequence.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub needs_order: Vec<String>,
    /// Explicit goal flag.
    pub goal: Option<bool>,
    /// Matrix cell fan-out flag.
    pub matrix: Option<bool>,
}

/// Result of CI intent DAG resolution.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CiResolveResult {
    /// Ordered list of tool targets to execute.
    pub runs: Vec<String>,
    /// Execution counts per tool (1 for source/join, N for cells).
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub counts: BTreeMap<String, usize>,
    /// Arguments bound to tools.
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub args: BTreeMap<String, String>,
    /// Strict execution ordering pairs `[first, second]`.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub before: Vec<(String, String)>,
    /// Error code if rejected (e.g., `"cycle"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reject: Option<String>,
}

impl CiResolveResult {
    /// Creates a rejected result with the specified error code.
    #[must_use]
    pub fn rejected(code: impl Into<String>) -> Self {
        Self {
            runs: Vec::new(),
            counts: BTreeMap::new(),
            args: BTreeMap::new(),
            before: Vec::new(),
            reject: Some(code.into()),
        }
    }
}

/// Normalizes a `needs` field into `(name, option)` pairs preserving sequence order for YAML arrays.
pub(crate) fn normalize_needs_ordered(val: Option<&Value>) -> Vec<(String, CiNeedOption)> {
    let mut pairs = Vec::new();
    let Some(v) = val else {
        return pairs;
    };

    match v {
        Value::String(s) => {
            pairs.push((
                s.clone(),
                CiNeedOption {
                    enabled: true,
                    args: None,
                },
            ));
        }
        Value::Array(arr) => {
            for item in arr {
                if let Value::String(s) = item {
                    pairs.push((
                        s.clone(),
                        CiNeedOption {
                            enabled: true,
                            args: None,
                        },
                    ));
                }
            }
        }
        Value::Object(obj) => {
            for (k, opt) in obj {
                match opt {
                    Value::Bool(b) => {
                        pairs.push((
                            k.clone(),
                            CiNeedOption {
                                enabled: *b,
                                args: None,
                            },
                        ));
                    }
                    Value::Object(inner) => {
                        let enabled = inner
                            .get("enabled")
                            .and_then(|e| e.as_bool())
                            .unwrap_or(true);
                        let args = inner
                            .get("args")
                            .and_then(|a| a.as_str())
                            .map(ToString::to_string);
                        pairs.push((k.clone(), CiNeedOption { enabled, args }));
                    }
                    Value::Null => {
                        pairs.push((
                            k.clone(),
                            CiNeedOption {
                                enabled: false,
                                args: None,
                            },
                        ));
                    }
                    _ => {
                        pairs.push((
                            k.clone(),
                            CiNeedOption {
                                enabled: true,
                                args: None,
                            },
                        ));
                    }
                }
            }
        }
        _ => {}
    }

    pairs
}

/// Normalizes a `needs` field from string, array, or object into a map.
pub fn normalize_needs(val: Option<&Value>) -> BTreeMap<String, CiNeedOption> {
    normalize_needs_ordered(val).into_iter().collect()
}

/// Computes Cartesian product matrix cells filtering out excluded coordinates.
pub fn compute_matrix_cells(matrix_def: Option<&Value>) -> Vec<BTreeMap<String, String>> {
    let Some(m) = matrix_def else {
        return Vec::new();
    };
    let Some(axes_obj) = m.get("axes").and_then(|a| a.as_object()) else {
        return Vec::new();
    };
    if axes_obj.is_empty() {
        return Vec::new();
    }

    let mut axis_names = Vec::new();
    let mut axis_values: Vec<Vec<String>> = Vec::new();

    for (k, v) in axes_obj {
        axis_names.push(k.clone());
        let mut vals = Vec::new();
        if let Some(arr) = v.as_array() {
            for item in arr {
                match item {
                    Value::String(s) => vals.push(s.clone()),
                    Value::Number(n) => vals.push(n.to_string()),
                    Value::Bool(b) => vals.push(b.to_string()),
                    _ => vals.push(item.to_string()),
                }
            }
        }
        axis_values.push(vals);
    }

    let mut excludes = Vec::new();
    if let Some(exc_arr) = m.get("exclude").and_then(|e| e.as_array()) {
        for exc in exc_arr {
            if let Some(obj) = exc.as_object() {
                let mut map = BTreeMap::new();
                for (k, v) in obj {
                    let s = match v {
                        Value::String(st) => st.clone(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => v.to_string(),
                    };
                    map.insert(k.clone(), s);
                }
                excludes.push(map);
            }
        }
    }

    let combos = cartesian_product_values(&axis_values);
    let mut cells = Vec::new();

    for combo in combos {
        let mut cell = BTreeMap::new();
        for (name, val) in axis_names.iter().zip(combo) {
            cell.insert(name.clone(), val);
        }

        let mut excluded = false;
        for exc in &excludes {
            if exc.iter().all(|(k, v)| cell.get(k) == Some(v)) {
                excluded = true;
                break;
            }
        }

        if !excluded {
            cells.push(cell);
        }
    }

    cells
}

fn cartesian_product_values(lists: &[Vec<String>]) -> Vec<Vec<String>> {
    if lists.is_empty() {
        return vec![vec![]];
    }
    let mut result = vec![vec![]];
    for list in lists {
        let mut temp = Vec::new();
        for res_item in &result {
            for item in list {
                let mut new_item = res_item.clone();
                new_item.push(item.clone());
                temp.push(new_item);
            }
        }
        result = temp;
    }
    result
}

/// Zero-I/O CI Intent DAG Resolver.
pub struct CiResolver;

impl CiResolver {
    /// Resolves a full CI fixture with `includes` and `input`.
    #[must_use]
    pub fn resolve_fixture(fixture: &Value) -> CiResolveResult {
        Self::resolve_with(fixture, None)
    }

    fn resolve_with(fixture: &Value, only_goal: Option<&str>) -> CiResolveResult {
        let mut merged_nodes: BTreeMap<String, CiNode> = BTreeMap::new();

        // 1. Process includes in declaration order
        if let Some(includes) = fixture.get("includes").and_then(|i| i.as_array()) {
            for inc in includes {
                Self::merge_nodes_into(&mut merged_nodes, inc.get("nodes"));
            }
        }

        // 2. Process input nodes (local overrides)
        let input_val = fixture.get("input").unwrap_or(fixture);
        Self::merge_nodes_into(&mut merged_nodes, input_val.get("nodes"));

        // 3. Filter out disabled/false dependencies
        for node in merged_nodes.values_mut() {
            node.needs.retain(|_, opt| opt.enabled);
            node.needs_order.retain(|k| node.needs.contains_key(k));
            for k in node.needs.keys() {
                if !node.needs_order.contains(k) {
                    node.needs_order.push(k.clone());
                }
            }
        }

        let declared_nodes: BTreeSet<String> = merged_nodes.keys().cloned().collect();

        // 4. Cycle detection (DFS with 3 colors: 0=unvisited, 1=visiting, 2=visited)
        let mut visited: BTreeMap<String, u8> = BTreeMap::new();
        for node in &declared_nodes {
            if *visited.get(node).unwrap_or(&0) == 0
                && Self::has_cycle(node, &merged_nodes, &declared_nodes, &mut visited)
            {
                return CiResolveResult::rejected("cycle");
            }
        }

        // 5. Determine goals (explicit goal: true, or infer graph sinks)
        let flagged_goals: Vec<String> = merged_nodes
            .iter()
            .filter(|(_, d)| d.goal == Some(true))
            .map(|(n, _)| n.clone())
            .collect();

        let goals = if let Some(goal) = only_goal {
            if !declared_nodes.contains(goal) {
                return CiResolveResult::rejected("unknown-goal");
            }
            vec![goal.to_string()]
        } else if !flagged_goals.is_empty() {
            flagged_goals
        } else {
            let mut needed: BTreeSet<String> = BTreeSet::new();
            for d in merged_nodes.values() {
                for dep in d.needs.keys() {
                    if declared_nodes.contains(dep) {
                        needed.insert(dep.clone());
                    }
                }
            }
            declared_nodes
                .iter()
                .filter(|n| !needed.contains(*n))
                .cloned()
                .collect()
        };

        // 6. Reachable closure from goals
        let mut reachable_nodes: BTreeSet<String> = BTreeSet::new();
        let mut queue: VecDeque<String> = goals.into_iter().collect();

        while let Some(curr) = queue.pop_front() {
            if reachable_nodes.contains(&curr) {
                continue;
            }
            reachable_nodes.insert(curr.clone());
            if let Some(node_def) = merged_nodes.get(&curr) {
                for dep in node_def.needs.keys() {
                    if declared_nodes.contains(dep) && !reachable_nodes.contains(dep) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        // 7. Topological sort of reachable nodes
        let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut in_deg: BTreeMap<String, usize> = BTreeMap::new();

        for n in &reachable_nodes {
            adj.entry(n.clone()).or_default();
            in_deg.entry(n.clone()).or_insert(0);
        }

        for n in &reachable_nodes {
            if let Some(node_def) = merged_nodes.get(n) {
                for dep in node_def.needs.keys() {
                    if reachable_nodes.contains(dep) {
                        adj.entry(dep.clone()).or_default().push(n.clone());
                        *in_deg.entry(n.clone()).or_insert(0) += 1;
                    }
                }
            }
        }

        let mut zero_in: VecDeque<String> = reachable_nodes
            .iter()
            .filter(|n| *in_deg.get(*n).unwrap_or(&0) == 0)
            .cloned()
            .collect();

        let mut topo_nodes = Vec::new();
        while let Some(u) = zero_in.pop_front() {
            topo_nodes.push(u.clone());
            if let Some(neighbors) = adj.get(&u) {
                for v in neighbors {
                    if let Some(deg) = in_deg.get_mut(v) {
                        *deg -= 1;
                        if *deg == 0 {
                            zero_in.push_back(v.clone());
                        }
                    }
                }
            }
        }

        // 8. Identify timing classes (CELL, SOURCE, JOIN)
        let cell_nodes: BTreeSet<String> = reachable_nodes
            .iter()
            .filter(|n| merged_nodes.get(*n).and_then(|d| d.matrix) == Some(true))
            .cloned()
            .collect();

        let mut upstream_of_cells: BTreeSet<String> = BTreeSet::new();
        let mut q_up: VecDeque<String> = cell_nodes.iter().cloned().collect();
        while let Some(curr) = q_up.pop_front() {
            if let Some(node_def) = merged_nodes.get(&curr) {
                for dep in node_def.needs.keys() {
                    if reachable_nodes.contains(dep) && !upstream_of_cells.contains(dep) {
                        upstream_of_cells.insert(dep.clone());
                        q_up.push_back(dep.clone());
                    }
                }
            }
        }

        let mut downstream_of_cells: BTreeSet<String> = BTreeSet::new();
        let mut q_down: VecDeque<String> = cell_nodes.iter().cloned().collect();
        while let Some(curr) = q_down.pop_front() {
            if let Some(neighbors) = adj.get(&curr) {
                for v in neighbors {
                    if !downstream_of_cells.contains(v) {
                        downstream_of_cells.insert(v.clone());
                        q_down.push_back(v.clone());
                    }
                }
            }
        }

        let matrix_cells = compute_matrix_cells(input_val.get("matrix"));
        let num_cells = if matrix_cells.is_empty() {
            1
        } else {
            matrix_cells.len()
        };

        // 9. Collect tools in topological order (SPEC-0003 requirement 17 preserves sequence order)
        let mut tool_order: Vec<String> = Vec::new();
        let mut tool_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut tool_args: BTreeMap<String, String> = BTreeMap::new();
        let mut tool_nodes: BTreeMap<String, String> = BTreeMap::new();

        for n in &topo_nodes {
            if let Some(node_def) = merged_nodes.get(n) {
                let is_cell = cell_nodes.contains(n);
                let multiplier = if is_cell { num_cells } else { 1 };

                for dep in &node_def.needs_order {
                    let Some(opt) = node_def.needs.get(dep) else {
                        continue;
                    };
                    if !declared_nodes.contains(dep) {
                        let tool = dep.clone();
                        if !tool_order.contains(&tool) {
                            tool_order.push(tool.clone());
                            tool_nodes.insert(tool.clone(), n.clone());
                        }

                        *tool_counts.entry(tool.clone()).or_insert(0) += multiplier;

                        if let Some(ref a) = opt.args {
                            tool_args.insert(tool.clone(), a.clone());
                        }
                    }
                }
            }
        }

        // 10. Compute strict before ordering pairs
        let mut before_pairs: Vec<(String, String)> = Vec::new();
        for (i, t1) in tool_order.iter().enumerate() {
            for t2 in &tool_order[i + 1..] {
                let n1 = &tool_nodes[t1];
                let n2 = &tool_nodes[t2];

                if upstream_of_cells.contains(n1) && downstream_of_cells.contains(n2) {
                    before_pairs.push((t1.clone(), t2.clone()));
                } else if n1 != n2 {
                    // Check reachability from n1 to n2 in adj
                    let mut q_reach = VecDeque::from([n1.clone()]);
                    let mut visited_reach = BTreeSet::new();
                    let mut reaches = false;

                    while let Some(c) = q_reach.pop_front() {
                        if &c == n2 {
                            reaches = true;
                            break;
                        }
                        if let Some(neighbors) = adj.get(&c) {
                            for nxt in neighbors {
                                if visited_reach.insert(nxt.clone()) {
                                    q_reach.push_back(nxt.clone());
                                }
                            }
                        }
                    }

                    if reaches {
                        before_pairs.push((t1.clone(), t2.clone()));
                    }
                }
            }
        }

        CiResolveResult {
            runs: tool_order,
            counts: tool_counts,
            args: tool_args,
            before: before_pairs,
            reject: None,
        }
    }

    /// Resolves the needs closure of one named node only (SPEC-0003 requirements 11, 12).
    ///
    /// The `goal: true` flags are ignored. A name that is not a declared node is
    /// rejected with the code `unknown-goal`.
    #[must_use]
    pub fn resolve_goal(fixture: &Value, goal: &str) -> CiResolveResult {
        Self::resolve_with(fixture, Some(goal))
    }

    fn merge_nodes_into(dest: &mut BTreeMap<String, CiNode>, src: Option<&Value>) {
        let Some(nodes_obj) = src.and_then(|s| s.as_object()) else {
            return;
        };

        for (name, node_val) in nodes_obj {
            let entry = dest.entry(name.clone()).or_default();
            let ordered = normalize_needs_ordered(node_val.get("needs"));
            if matches!(node_val.get("needs"), Some(Value::Array(_))) {
                let seq_keys: Vec<String> = ordered.iter().map(|(k, _)| k.clone()).collect();
                entry.needs_order.retain(|k| !seq_keys.contains(k));
                for k in seq_keys {
                    if !entry.needs_order.contains(&k) {
                        entry.needs_order.push(k);
                    }
                }
            } else {
                for (k, _) in &ordered {
                    if !entry.needs_order.contains(k) {
                        entry.needs_order.push(k.clone());
                    }
                }
            }
            for (k, opt) in ordered {
                entry.needs.insert(k, opt);
            }

            if let Some(goal) = node_val.get("goal").and_then(|g| g.as_bool()) {
                entry.goal = Some(goal);
            }
            if let Some(matrix) = node_val.get("matrix").and_then(|m| m.as_bool()) {
                entry.matrix = Some(matrix);
            }
        }
    }

    fn has_cycle(
        node: &str,
        nodes: &BTreeMap<String, CiNode>,
        declared: &BTreeSet<String>,
        visited: &mut BTreeMap<String, u8>,
    ) -> bool {
        visited.insert(node.to_string(), 1);

        if let Some(def) = nodes.get(node) {
            for dep in def.needs.keys() {
                if declared.contains(dep) {
                    match visited.get(dep).copied().unwrap_or(0) {
                        1 => return true,
                        0 if Self::has_cycle(dep, nodes, declared, visited) => return true,
                        _ => {}
                    }
                }
            }
        }

        visited.insert(node.to_string(), 2);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn goals_doc() -> Value {
        json!({
            "nodes": {
                "formatted": { "needs": { "cargo-fmt": true } },
                "ci": { "goal": true, "needs": { "formatted": true, "cargo-test": true } },
                "install": { "needs": { "ci": true, "cargo-build-release": true } },
                "hygiene": { "needs": { "typos": true } }
            }
        })
    }

    // SPEC-0003 requirement 11: --goal resolves that node's closure only.
    #[test]
    fn resolve_goal_takes_the_closure_of_one_node() {
        let doc = goals_doc();
        let install = CiResolver::resolve_goal(&doc, "install");
        assert_eq!(install.reject, None);
        assert_eq!(
            install.runs,
            vec!["cargo-fmt", "cargo-test", "cargo-build-release"]
        );
        let hygiene = CiResolver::resolve_goal(&doc, "hygiene");
        assert_eq!(hygiene.runs, vec!["typos"]);
    }

    // SPEC-0003 requirement 11: the flagged goal does not leak into another goal.
    #[test]
    fn resolve_goal_ignores_the_flagged_goal() {
        let doc = goals_doc();
        let formatted = CiResolver::resolve_goal(&doc, "formatted");
        assert_eq!(formatted.runs, vec!["cargo-fmt"]);
        let ci = CiResolver::resolve_goal(&doc, "ci");
        assert!(!ci.runs.contains(&"cargo-build-release".to_string()));
        assert_eq!(ci.runs, CiResolver::resolve_fixture(&doc).runs);
    }

    // SPEC-0003 requirement 12: an undeclared goal is rejected.
    #[test]
    fn resolve_goal_rejects_an_unknown_node() {
        let doc = goals_doc();
        let res = CiResolver::resolve_goal(&doc, "no-such-node");
        assert_eq!(res.reject.as_deref(), Some("unknown-goal"));
        assert!(res.runs.is_empty());
        let tool = CiResolver::resolve_goal(&doc, "cargo-fmt");
        assert_eq!(tool.reject.as_deref(), Some("unknown-goal"));
    }

    // SPEC-0003 requirement 17: a YAML sequence in `needs` keeps declared order within a node.
    #[test]
    fn sequence_needs_keeps_declared_order_within_a_node() {
        let doc = json!({
            "nodes": {
                "hygiene": {
                    "needs": ["toolchain-check", "projectfile-check", "spec-check", "stages-check", "zero-io-domain"]
                },
                "check": {
                    "goal": true,
                    "needs": ["hygiene", "cargo-fmt", "cargo-clippy", "cargo-test"]
                }
            }
        });
        let res = CiResolver::resolve_fixture(&doc);
        assert_eq!(res.reject, None);
        assert_eq!(
            res.runs,
            vec![
                "toolchain-check",
                "projectfile-check",
                "spec-check",
                "stages-check",
                "zero-io-domain",
                "cargo-fmt",
                "cargo-clippy",
                "cargo-test",
            ]
        );
    }
}
