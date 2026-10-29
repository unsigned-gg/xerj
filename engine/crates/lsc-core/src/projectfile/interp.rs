// SPDX-License-Identifier: Apache-2.0 OR MIT
//! §3.8 projectfile v1 Interpolation Engine.
//!
//! Conforms to normative specification <https://projectfile.org/spec/v1> (§3.8)
//! and conformance vectors under `spec/conformance/interpolation/*.yaml`.
//!
//! # Invariants
//! - Zero-I/O: operates purely on memory buffers and `serde_json::Value` / `serde_yaml::Value`.
//! - Balanced braces and brackets for nested expressions.
//! - Escaped `$${...}` and `$$[...]` preservation.

use serde_json::Value;

/// Maximum recursive reference resolution depth to prevent cyclic recursion.
pub const MAX_INTERPOLATION_DEPTH: usize = 10;

/// Represents a parsed reference within a string template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceSpan {
    /// Start byte offset in original text.
    pub start: usize,
    /// End byte offset in original text.
    pub end: usize,
    /// Full matched string including `${` or `$${` and trailing `}`.
    pub raw: String,
    /// Inner expression content.
    pub inner: String,
    /// True if escaped with leading `$$`.
    pub is_escaped: bool,
}

/// Represents a parsed span `$[address | template]` or `$[?address | template]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateSpan {
    /// Start byte offset in original text.
    pub start: usize,
    /// End byte offset in original text.
    pub end: usize,
    /// Full matched string including `$[...]` and trailing space.
    pub raw: String,
    /// True if optional span prefix `?`.
    pub is_optional: bool,
    /// Queried field address.
    pub address: String,
    /// Repeated item template.
    pub template: String,
    /// Trailing space following `]` if present.
    pub trailing_space: String,
}

/// Finds all `${...}` and `$${...}` references with balanced braces.
pub fn find_references(text: &str) -> Vec<ReferenceSpan> {
    let mut results = Vec::new();
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = 0;

    while i < n {
        if i + 2 < n && &bytes[i..i + 3] == b"$${" {
            let start = i;
            i += 3;
            let mut depth = 1usize;
            let inner_start = i;
            while i < n && depth > 0 {
                match bytes[i] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            if depth == 0 {
                let inner = &text[inner_start..i - 1];
                results.push(ReferenceSpan {
                    start,
                    end: i,
                    raw: text[start..i].to_string(),
                    inner: inner.to_string(),
                    is_escaped: true,
                });
            }
        } else if i + 1 < n && &bytes[i..i + 2] == b"${" {
            let start = i;
            i += 2;
            let mut depth = 1usize;
            let inner_start = i;
            while i < n && depth > 0 {
                match bytes[i] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            if depth == 0 {
                let inner = &text[inner_start..i - 1];
                results.push(ReferenceSpan {
                    start,
                    end: i,
                    raw: text[start..i].to_string(),
                    inner: inner.to_string(),
                    is_escaped: false,
                });
            }
        } else {
            i += 1;
        }
    }

    results
}

/// Finds all `$[...]` and `$[?...]` spans with balanced brackets.
pub fn find_spans(text: &str) -> Vec<TemplateSpan> {
    let mut results = Vec::new();
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = 0;

    while i < n {
        if i + 1 < n && &bytes[i..i + 2] == b"$[" {
            let start = i;
            i += 2;
            let mut is_optional = false;
            if i < n && bytes[i] == b'?' {
                is_optional = true;
                i += 1;
            }
            let inner_start = i;
            let mut depth = 1usize;
            while i < n && depth > 0 {
                match bytes[i] {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            if depth == 0 {
                let span_end = i;
                let mut trailing_space = String::new();
                if i < n && bytes[i] == b' ' {
                    trailing_space.push(' ');
                    i += 1;
                }
                let inner = &text[inner_start..span_end - 1];
                if let Some((addr, tmpl)) = inner.split_once('|') {
                    results.push(TemplateSpan {
                        start,
                        end: i,
                        raw: text[start..i].to_string(),
                        is_optional,
                        address: addr.trim().to_string(),
                        template: tmpl.trim().to_string(),
                        trailing_space,
                    });
                }
            }
        } else {
            i += 1;
        }
    }

    results
}

/// Filter transformation for references (`| env`).
pub fn apply_filter(val: &str, filter: &str) -> Option<String> {
    match filter {
        "env" => Some(val.to_ascii_uppercase().replace(['.', '-'], "_")),
        _ => None,
    }
}

/// §3.8 Interpolator engine operating over in-memory JSON document models.
#[derive(Debug, Clone)]
pub struct ProjectfileInterpolator<'a> {
    doc: &'a Value,
    max_depth: usize,
}

impl<'a> ProjectfileInterpolator<'a> {
    /// Creates a new interpolator for the given document root.
    pub fn new(doc: &'a Value) -> Self {
        Self {
            doc,
            max_depth: MAX_INTERPOLATION_DEPTH,
        }
    }

    /// Resolves one step in an address path against a current Value node.
    pub fn resolve_path_step<'v>(
        current: &'v Value,
        step: &str,
    ) -> Vec<(&'v Value, Option<&'v Value>)> {
        // List selector: key[selector]
        if let Some(open) = step.find('[') {
            if step.ends_with(']') {
                let key = &step[..open];
                let selector = &step[open + 1..step.len() - 1];
                let val = match current {
                    Value::Object(map) => map.get(key),
                    _ => None,
                };
                if let Some(Value::Array(arr)) = val {
                    if selector.is_empty() {
                        return arr
                            .iter()
                            .map(|item| (item, item.as_object().map(|_| item)))
                            .collect();
                    } else if let Ok(idx) = selector.parse::<isize>() {
                        let actual_idx = if idx < 0 {
                            arr.len() as isize + idx
                        } else {
                            idx
                        };
                        if actual_idx >= 0 && (actual_idx as usize) < arr.len() {
                            let item = &arr[actual_idx as usize];
                            return vec![(item, item.as_object().map(|_| item))];
                        }
                        return Vec::new();
                    } else {
                        let terms: Vec<&str> = selector
                            .split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .collect();
                        for item in arr {
                            if Self::matches_terms(item, &terms) {
                                return vec![(item, item.as_object().map(|_| item))];
                            }
                        }
                        return Vec::new();
                    }
                }
                return Vec::new();
            }
        }

        // Map selector: key{selector}
        if let Some(open) = step.find('{') {
            if step.ends_with('}') {
                let key = &step[..open];
                let selector = &step[open + 1..step.len() - 1];
                let val = match current {
                    Value::Object(map) => map.get(key),
                    _ => None,
                };
                if let Some(Value::Object(map)) = val {
                    let terms: Vec<&str> = selector
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .collect();
                    let mut results = Vec::new();
                    for (_k, item) in map {
                        if Self::matches_terms(item, &terms) {
                            results.push((item, item.as_object().map(|_| item)));
                        }
                    }
                    return results;
                }
                return Vec::new();
            }
        }

        // Bare key
        match current {
            Value::Object(map) => {
                if let Some(v) = map.get(step) {
                    vec![(v, v.as_object().map(|_| v))]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    fn matches_terms(item: &Value, terms: &[&str]) -> bool {
        let map = match item {
            Value::Object(m) => m,
            _ => return false,
        };
        for term in terms {
            if let Some((k, expected)) = term.split_once('=') {
                let k = k.trim();
                let expected = expected.trim();
                let actual = match map.get(k) {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Number(n)) => n.to_string(),
                    Some(Value::Bool(b)) => b.to_string(),
                    _ => String::new(),
                };
                if actual != expected {
                    return false;
                }
            } else if let Some(stripped) = term.strip_prefix('!') {
                let k = stripped.trim();
                if let Some(val) = map.get(k) {
                    if !Self::is_empty_or_falsy(val) {
                        return false;
                    }
                }
            } else {
                let k = term.trim();
                match map.get(k) {
                    Some(val) if !Self::is_empty_or_falsy(val) => {}
                    _ => return false,
                }
            }
        }
        true
    }

    fn is_empty_or_falsy(val: &Value) -> bool {
        match val {
            Value::Null => true,
            Value::Bool(b) => !*b,
            Value::String(s) => s.is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            Value::Number(_) => false,
        }
    }

    /// Queries an address against scope (if present) and document root.
    pub fn query_address(&self, address: &str, scope: Option<&'a Value>) -> Vec<&'a Value> {
        let address = address.trim();
        if address == "." {
            if let Some(sc) = scope {
                return vec![sc];
            }
            return Vec::new();
        }

        // Split address by dots taking care of nested [] and {}
        let mut parts = Vec::new();
        let mut curr = String::new();
        let mut depth_sq = 0usize;
        let mut depth_cur = 0usize;

        for ch in address.chars() {
            match ch {
                '[' => {
                    depth_sq += 1;
                    curr.push(ch);
                }
                ']' => {
                    depth_sq = depth_sq.saturating_sub(1);
                    curr.push(ch);
                }
                '{' => {
                    depth_cur += 1;
                    curr.push(ch);
                }
                '}' => {
                    depth_cur = depth_cur.saturating_sub(1);
                    curr.push(ch);
                }
                '.' if depth_sq == 0 && depth_cur == 0 => {
                    parts.push(curr.clone());
                    curr.clear();
                }
                _ => curr.push(ch),
            }
        }
        if !curr.is_empty() {
            parts.push(curr);
        }

        let mut roots = Vec::new();
        if let Some(sc) = scope {
            roots.push(sc);
        }
        roots.push(self.doc);

        for root in roots {
            let matches = self.resolve_parts(root, &parts);
            if !matches.is_empty() {
                return matches;
            }
        }

        Vec::new()
    }

    fn resolve_parts(&self, root: &'a Value, parts: &[String]) -> Vec<&'a Value> {
        let mut current_candidates: Vec<(&'a Value, Option<&'a Value>)> =
            vec![(root, root.as_object().map(|_| root))];

        let mut i = 0;
        while i < parts.len() {
            let mut found = false;
            for j in (i + 1..=parts.len()).rev() {
                let composite_step = parts[i..j].join(".");
                let mut next_candidates = Vec::new();
                for (val, _sc) in &current_candidates {
                    let res = Self::resolve_path_step(val, &composite_step);
                    next_candidates.extend(res);
                }
                if !next_candidates.is_empty() {
                    current_candidates = next_candidates;
                    i = j;
                    found = true;
                    break;
                }
            }
            if !found {
                return Vec::new();
            }
        }

        current_candidates.into_iter().map(|(val, _)| val).collect()
    }

    /// Evaluates string interpolation in scalar 'out' mode.
    pub fn interpolate_string(&self, text: &str, scope: Option<&'a Value>, depth: usize) -> String {
        if depth > self.max_depth {
            return text.to_string();
        }

        // Protect escaped spans
        let protected = text.replace("$$[", "\x00ESCAPED_SPAN\x00");

        // Resolve spans
        let with_spans = self.interpolate_spans(&protected, scope, depth);

        // Find references
        let refs = find_references(&with_spans);
        if refs.is_empty() {
            return with_spans.replace("\x00ESCAPED_SPAN\x00", "$[");
        }

        let mut res = with_spans;
        for r in refs.into_iter().rev() {
            let replacement = if r.is_escaped {
                r.raw[1..].to_string()
            } else {
                let (addr_part, filter_name) = match r.inner.split_once('|') {
                    Some((a, f)) => (a.trim(), Some(f.trim())),
                    None => (r.inner.as_str(), None),
                };

                if let Some(flt) = filter_name {
                    if flt != "env" {
                        r.raw.clone()
                    } else {
                        let matches = self.query_address(addr_part, scope);
                        if matches.len() != 1 {
                            r.raw.clone()
                        } else {
                            let val = matches[0];
                            if val.is_object() || val.is_array() {
                                r.raw.clone()
                            } else {
                                let raw_str = Self::value_to_str(val);
                                let flt_str = apply_filter(&raw_str, flt).unwrap_or(raw_str);
                                if flt_str.contains("${") {
                                    self.interpolate_string(&flt_str, scope, depth + 1)
                                } else {
                                    flt_str
                                }
                            }
                        }
                    }
                } else {
                    let matches = self.query_address(addr_part, scope);
                    if matches.len() != 1 {
                        r.raw.clone()
                    } else {
                        let val = matches[0];
                        if val.is_object() || val.is_array() {
                            r.raw.clone()
                        } else {
                            let mut val_str = Self::value_to_str(val);
                            if val_str.contains("${") {
                                val_str = self.interpolate_string(&val_str, scope, depth + 1);
                            }
                            val_str
                        }
                    }
                }
            };

            res = format!("{}{}{}", &res[..r.start], replacement, &res[r.end..]);
        }

        res.replace("\x00ESCAPED_SPAN\x00", "$[")
    }

    fn interpolate_spans(&self, text: &str, scope: Option<&'a Value>, depth: usize) -> String {
        let spans = find_spans(text);
        if spans.is_empty() {
            return text.to_string();
        }

        let mut res = text.to_string();
        for sp in spans.into_iter().rev() {
            let results = self.query_address(&sp.address, scope);
            let replacement = if results.is_empty() {
                if sp.is_optional {
                    String::new()
                } else {
                    sp.raw.clone()
                }
            } else {
                let mut rendered_items = Vec::new();
                let mut aborted = false;

                for item in results {
                    if let Some(rendered) =
                        self.interpolate_template_item(&sp.template, item, depth)
                    {
                        rendered_items.push(rendered);
                    } else {
                        aborted = true;
                        break;
                    }
                }

                if aborted {
                    sp.raw.clone()
                } else {
                    format!("{}{}", rendered_items.join(" "), sp.trailing_space)
                }
            };

            res = format!("{}{}{}", &res[..sp.start], replacement, &res[sp.end..]);
        }

        res
    }

    fn interpolate_template_item(
        &self,
        template: &str,
        item: &'a Value,
        _depth: usize,
    ) -> Option<String> {
        let refs = find_references(template);
        if refs.is_empty() {
            return Some(template.to_string());
        }

        let mut res = template.to_string();
        for r in refs.into_iter().rev() {
            let rep = if r.is_escaped {
                r.raw[1..].to_string()
            } else {
                let (addr_part, filter_name) = match r.inner.split_once('|') {
                    Some((a, f)) => (a.trim(), Some(f.trim())),
                    None => (r.inner.as_str(), None),
                };

                let val = if addr_part == "." {
                    item
                } else if let Some(obj) = item.as_object() {
                    if let Some(v) = obj.get(addr_part) {
                        v
                    } else {
                        let q = self.query_address(addr_part, Some(item));
                        if q.len() == 1 {
                            q[0]
                        } else {
                            return None;
                        }
                    }
                } else {
                    let q = self.query_address(addr_part, Some(item));
                    if q.len() == 1 {
                        q[0]
                    } else {
                        return None;
                    }
                };

                if val.is_object() || val.is_array() {
                    return None;
                }

                let mut val_str = Self::value_to_str(val);
                if let Some(flt) = filter_name {
                    val_str = apply_filter(&val_str, flt)?;
                }
                val_str
            };

            res = format!("{}{}{}", &res[..r.start], rep, &res[r.end..]);
        }

        Some(res)
    }

    /// Evaluates multi-value references emitting lines with Cartesian product.
    pub fn interpolate_lines(&self, text: &str) -> Vec<String> {
        let refs = find_references(text);
        let mut multi_refs: Vec<(ReferenceSpan, Vec<String>)> = Vec::new();

        for r in refs {
            if !r.is_escaped {
                let addr = r.inner.split('|').next().unwrap_or("").trim();
                let results = self.query_address(addr, None);
                let scalar_results: Vec<String> = results
                    .iter()
                    .filter(|x| !x.is_object() && !x.is_array())
                    .map(|x| Self::value_to_str(x))
                    .collect();

                if results.len() > 1 && !scalar_results.is_empty() {
                    multi_refs.push((r, scalar_results));
                } else if results.is_empty() {
                    multi_refs.push((r.clone(), vec![r.raw.clone()]));
                } else {
                    let interpolated = self.interpolate_string(&r.raw, None, 0);
                    multi_refs.push((r, vec![interpolated]));
                }
            } else {
                multi_refs.push((r.clone(), vec![r.raw[1..].to_string()]));
            }
        }

        if !multi_refs.iter().any(|(_, choices)| choices.len() > 1) {
            return vec![self.interpolate_string(text, None, 0)];
        }

        let choices_list: Vec<Vec<String>> = multi_refs.iter().map(|(_, c)| c.clone()).collect();
        let combos = cartesian_product(&choices_list);

        let mut lines = Vec::new();
        for combo in combos {
            let mut curr_line = text.to_string();
            for ((r, _), chosen_val) in multi_refs.iter().zip(combo.iter()).rev() {
                curr_line = format!(
                    "{}{}{}",
                    &curr_line[..r.start],
                    chosen_val,
                    &curr_line[r.end..]
                );
            }
            lines.push(self.interpolate_string(&curr_line, None, 0));
        }

        lines
    }

    fn value_to_str(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
            _ => v.to_string(),
        }
    }
}

fn cartesian_product(lists: &[Vec<String>]) -> Vec<Vec<String>> {
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
