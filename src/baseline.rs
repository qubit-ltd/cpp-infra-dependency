// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Direct requirement identity and strict three-part resolved-version policy.
use std::collections::BTreeMap;

use serde::Deserialize;

use crate::dependency_error::DependencyError;
use crate::graph::Graph;
use crate::graph::reference_parts;
use crate::option_configuration::OptionConfiguration;

type Result<T> = std::result::Result<T, DependencyError>;
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Baseline {
    pub schema: u32,
    pub baseline_id: String,
    #[serde(default)]
    pub internal_patterns: Vec<String>,
    #[serde(default)]
    pub allow_refs: Vec<String>,
    #[serde(default)]
    pub direct: BTreeMap<String, String>,
    #[serde(default)]
    pub resolved: BTreeMap<String, String>,
    #[serde(default)]
    pub remotes: BTreeMap<String, String>,
}
impl Baseline {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema != 1 || self.baseline_id.is_empty() {
            return Err(DependencyError::Configuration(
                "baseline requires schema=1 and baseline_id".into(),
            ));
        }
        for (name, reference) in &self.direct {
            let (actual, _, _, _, _) = reference_parts(reference)?;
            if name != &actual {
                return Err(DependencyError::Configuration(format!(
                    "baseline direct key differs from require name: {name}"
                )));
            }
        }
        for reference in &self.allow_refs {
            reference_parts(reference)?;
        }
        for range in self.resolved.values() {
            parse_range(range)?;
        }
        for pattern in &self.internal_patterns {
            if pattern.is_empty() || pattern.matches('*').count() > 1 || pattern.contains('?') || pattern.contains('[')
            {
                return Err(DependencyError::Configuration(format!(
                    "unsupported internal name pattern: {pattern}"
                )));
            }
        }
        for (name, url) in &self.remotes {
            if name.is_empty()
                || !(url.starts_with("https://") || url.starts_with("http://"))
                || url.contains('@')
                || url.contains('?')
                || url.contains('#')
            {
                return Err(DependencyError::Configuration(format!(
                    "remote {name} needs an HTTP(S) URL without credentials/query/fragment"
                )));
            }
        }
        Ok(())
    }
    pub(crate) fn internal(&self, name: &str) -> bool {
        self.internal_patterns.iter().any(|pattern| {
            if let Some((start, end)) = pattern.split_once('*') {
                name.starts_with(start) && name.ends_with(end) && name.len() >= start.len() + end.len()
            } else {
                name == pattern
            }
        })
    }
    pub(crate) fn evaluate(&self, graph: &Graph) -> Result<()> {
        let mut failures = Vec::new();
        for requirement in &graph.direct {
            if self.internal(&requirement.name) {
                continue;
            }
            match self.direct.get(&requirement.name) {
                None => failures.push(format!(
                    "unregistered external direct {} {} ({})",
                    requirement.category, requirement.requirement, requirement.context
                )),
                Some(canonical) if canonical != &requirement.requirement => failures.push(format!(
                    "direct {} must use {canonical}, got {}",
                    requirement.name, requirement.requirement
                )),
                _ => {}
            }
        }
        for node in &graph.nodes {
            let exact = self
                .allow_refs
                .iter()
                .any(|r| r == &node.reference || r == node.reference.split('#').next().unwrap_or(&node.reference));
            let version = parse_version(&node.version);
            if version.is_none() && !exact {
                failures.push(format!(
                    "resolved {} in {} uses unsupported version; register exact allow_refs",
                    node.reference, node.context
                ));
                continue;
            }
            if let Some(range) = self.resolved.get(&node.name)
                && !exact
                && !version.is_some_and(|v| {
                    parse_range(range)
                        .is_ok_and(|comparisons| comparisons.iter().all(|(op, expected)| compare(&v, op, expected)))
                })
            {
                failures.push(format!(
                    "resolved {} in {} violates {range}",
                    node.reference, node.context
                ));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(DependencyError::Gate(failures.join("; ")))
        }
    }
}
fn parse_version(version: &str) -> Option<[u64; 3]> {
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) || (p.len() > 1 && p.starts_with('0')))
    {
        return None;
    }
    Some([parts[0].parse().ok()?, parts[1].parse().ok()?, parts[2].parse().ok()?])
}
fn parse_range(range: &str) -> Result<Vec<(String, [u64; 3])>> {
    let mut result = Vec::new();
    for part in range.split(',').map(str::trim) {
        let (op, version) = [">=", "<=", "==", ">", "<", "="]
            .iter()
            .find_map(|op| part.strip_prefix(op).map(|v| (*op, v)))
            .unwrap_or(("=", part));
        let version = parse_version(version.trim()).required_configuration(format_args!(
            "resolved range accepts only numeric three-part comparisons: {range}"
        ))?;
        result.push((op.into(), version));
    }
    Ok(result)
}
fn compare(actual: &[u64; 3], op: &str, expected: &[u64; 3]) -> bool {
    match op {
        ">=" => actual >= expected,
        "<=" => actual <= expected,
        ">" => actual > expected,
        "<" => actual < expected,
        _ => actual == expected,
    }
}
