// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Locked Conan command construction and JSON-backed remote verification.
use std::fs;
use std::path::Path;

use serde_json::Value;
use serde_json::json;

use crate::baseline::Baseline;
use crate::config::Config;
use crate::config::Context;
use crate::dependency_error::DependencyError;
use crate::digest::project_path;
use crate::graph::Graph;
use crate::option_configuration::OptionConfiguration;
use crate::report::Report;
use crate::report::atomic_json;
use crate::result_configuration::ResultConfiguration;
use crate::runner::conan_json;

type Result<T> = std::result::Result<T, DependencyError>;
pub(crate) fn arguments(
    project: &Path,
    config: &Config,
    context: &Context,
    offline: bool,
    recipe: &Path,
    lock: Option<&Path>,
) -> Result<Vec<String>> {
    let mut args = vec![
        recipe.to_string_lossy().into_owned(),
        "--profile:build".into(),
        project_path(project, &context.build_profile)?
            .to_string_lossy()
            .into_owned(),
        "--profile:host".into(),
        project_path(project, &context.host_profile)?
            .to_string_lossy()
            .into_owned(),
    ];
    if let Some(lock) = lock {
        args.extend(["--lockfile".into(), lock.to_string_lossy().into_owned()]);
    } else {
        args.push("--lockfile=".into());
    }
    if offline || context.remotes.is_empty() {
        args.push("--no-remote".into());
    } else {
        for remote in &context.remotes {
            args.extend(["--remote".into(), remote.clone()]);
        }
    }
    args.extend(["--conf:host".into(), "tools.cmake.cmaketoolchain:user_presets=".into()]);
    let _ = config;
    Ok(args)
}
pub(crate) fn verify_remotes(
    project: &Path,
    config: &Config,
    context: &Context,
    baseline: &Baseline,
    offline: bool,
    report: &mut Report,
) -> Result<()> {
    if offline || context.remotes.is_empty() {
        report.value["data"]["remotes"] = json!([]);
        return Ok(());
    }
    let value = conan_json(
        report,
        project,
        &["remote".into(), "list".into(), "--format=json".into()],
        config.timeout_seconds,
    )?;
    let remotes = value
        .as_array()
        .required_configuration("unsupported Conan remote list JSON")?;
    let mut approved = Vec::new();
    for name in &context.remotes {
        let expected = baseline
            .remotes
            .get(name)
            .required_configuration(format_args!("unregistered remote: {name}"))?;
        let remote = remotes
            .iter()
            .find(|r| r["name"].as_str() == Some(name))
            .required_configuration(format_args!("Conan remote is missing: {name}"))?;
        if remote["url"].as_str() != Some(expected) || remote["enabled"].as_bool() == Some(false) {
            return Err(DependencyError::Configuration(format!(
                "Conan remote {name} URL/enabled state differs from policy"
            )));
        }
        approved.push(json!({"name":name,"url":expected}));
    }
    report.value["data"]["remotes"] = json!(approved);
    Ok(())
}
pub(crate) fn graph_info(
    project: &Path,
    config: &Config,
    context: &Context,
    offline: bool,
    recipe: &Path,
    lock: &Path,
    report: &mut Report,
) -> Result<(Graph, Value)> {
    let mut args = vec!["graph".into(), "info".into()];
    args.extend(arguments(project, config, context, offline, recipe, Some(lock))?);
    args.push("--format=json".into());
    let value = conan_json(report, project, &args, config.timeout_seconds)?;
    let graph = Graph::parse(&value)?;
    Ok((graph, value))
}
pub(crate) fn require_locks(project: &Path, config: &Config) -> Result<()> {
    for (name, context) in &config.contexts {
        if !project_path(project, &context.lockfile)?.is_file() {
            return Err(DependencyError::Configuration(format!(
                "missing lock for context {name}; run lock update --context {name}"
            )));
        }
    }
    Ok(())
}
pub(crate) fn validate_lock(path: &Path, graph: &Graph) -> Result<()> {
    let value: Value = serde_json::from_slice(&fs::read(path)?)
        .configuration_context(format_args!("invalid Conan lock {}", path.display()))?;
    if value["version"].as_str() != Some("0.5") {
        return Err(DependencyError::Configuration(
            "unsupported Conan lock schema; expected version 0.5".into(),
        ));
    }
    for category in ["requires", "build_requires", "python_requires"] {
        let entries = value[category]
            .as_array()
            .required_configuration(format_args!("Conan lock missing {category}"))?;
        for entry in entries {
            if entry.as_str().is_none_or(|s| !s.contains('#')) {
                return Err(DependencyError::Configuration(format!(
                    "Conan lock {category} entry must pin a recipe revision"
                )));
            }
        }
    }
    for node in &graph.nodes {
        let category = if node.context == "build" {
            "build_requires"
        } else {
            "requires"
        };
        let expected = &node.reference;
        let covered = value[category].as_array().is_some_and(|entries| {
            entries.iter().any(|v| {
                v.as_str()
                    .is_some_and(|s| s.split('%').next() == Some(expected.as_str()))
            })
        });
        if !covered {
            return Err(DependencyError::Gate(format!(
                "lock does not cover {} in {}; run lock update",
                node.reference, node.context
            )));
        }
        if let Some(requires) = node.python_requires.as_object() {
            for dependency in requires.values() {
                if let Some(reference) = dependency["ref"].as_str()
                    && !value["python_requires"].as_array().is_some_and(|entries| {
                        entries
                            .iter()
                            .any(|v| v.as_str().is_some_and(|s| s.split('%').next() == Some(reference)))
                    })
                {
                    return Err(DependencyError::Gate(format!(
                        "lock does not cover Python require {reference}"
                    )));
                }
            }
        }
    }
    Ok(())
}
pub(crate) fn save_graph(report: &mut Report, graph: &Graph, raw: &Value) -> Result<()> {
    let graph_path = report.work.join("graph.json");
    let inventory_path = report.work.join("inventory.json");
    atomic_json(&graph_path, raw)?;
    let inventory = serde_json::to_value(graph).configuration()?;
    atomic_json(&inventory_path, &inventory)?;
    report.artifact("dependency-graph", &graph_path, true)?;
    report.artifact("dependency-inventory", &inventory_path, true)?;
    report.value["data"]["inventory"] = inventory;
    Ok(())
}
