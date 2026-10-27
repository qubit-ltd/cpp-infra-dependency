// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Conservative text-recipe synchronization validated in a staging project.
use std::fs;
use std::path::Path;

use serde_json::json;
use tempfile::tempdir_in;

use crate::baseline::Baseline;
use crate::conan_adapter::arguments;
use crate::config::Config;
use crate::dependency_error::DependencyError;
use crate::digest::project_digests;
use crate::digest::project_path;
use crate::digest::sha256;
use crate::graph::Graph;
use crate::graph::reference_parts;
use crate::option_configuration::OptionConfiguration;
use crate::project_lock::acquire;
use crate::report::Report;
use crate::report::atomic_write;
use crate::result_configuration::ResultConfiguration;
use crate::runner::conan_json;

type Result<T> = std::result::Result<T, DependencyError>;
pub(crate) fn sync(
    project: &Path,
    config: &Config,
    baseline: &Baseline,
    offline: bool,
    dry_run: bool,
    report: &mut Report,
) -> Result<()> {
    let recipe = project_path(project, &config.recipe)?;
    if recipe.file_name().and_then(|s| s.to_str()) != Some("conanfile.txt") {
        return Err(DependencyError::Configuration(format!(
            "Python recipes require a manually reviewed patch; suggested canonical requires: {:?}",
            baseline.direct
        )));
    }
    let before = fs::read_to_string(&recipe)?;
    let after = rewrite(&before, baseline)?;
    report.value["data"]["sync"] = json!({"dry_run":dry_run,"changed":before!=after,"before_sha256":sha256(before.as_bytes()),"after_sha256":sha256(after.as_bytes()),"proposed_recipe":after,"requires_lock_update":before!=after});
    if before == after {
        return Ok(());
    }
    fs::create_dir_all(&report.work)?;
    let stage = tempdir_in(&report.work)?;
    copy_project(project, project, stage.path())?;
    atomic_write(&stage.path().join(&config.recipe), after.as_bytes())?;
    for context in config.contexts.values() {
        let mut args = vec!["graph".into(), "info".into()];
        args.extend(arguments(
            stage.path(),
            config,
            context,
            offline,
            &stage.path().join(&config.recipe),
            None,
        )?);
        args.push("--format=json".into());
        let raw = conan_json(report, stage.path(), &args, config.timeout_seconds)?;
        baseline.evaluate(&Graph::parse(&raw)?)?;
    }
    let _guard = acquire(
        &project_path(project, "build/infra/locks/dependency-sync.lock")?,
        config.timeout_seconds,
    )?;
    let (source, config_digest) = project_digests(project, Some(&report.root), &report.source_excluded_paths)?;
    if report.value["source_digest"] != source || report.value["config_digest"] != config_digest {
        return Err(DependencyError::Configuration(
            "project inputs changed during sync validation; no replacement performed".into(),
        ));
    }
    if fs::read_to_string(&recipe)? != before {
        return Err(DependencyError::Configuration(
            "recipe changed during sync validation; no replacement performed".into(),
        ));
    }
    if !dry_run {
        atomic_write(&recipe, after.as_bytes())?;
    }
    report.diagnostic(
        "lock-refresh-required",
        "Recipe synchronization does not refresh locks; run lock update for each changed context before check/prepare.",
        "warning",
    );
    Ok(())
}
fn rewrite(text: &str, baseline: &Baseline) -> Result<String> {
    let mut section = String::new();
    let mut result = String::new();
    for raw in text.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\r', '\n']);
        let ending = &raw[line.len()..];
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if !trimmed.ends_with(']') {
                return Err(DependencyError::Configuration(
                    "ambiguous Conan text section; refusing sync".into(),
                ));
            }
            section = trimmed.to_string();
            result.push_str(raw);
            continue;
        }
        if !matches!(section.as_str(), "[requires]" | "[tool_requires]")
            || trimmed.is_empty()
            || trimmed.starts_with('#')
        {
            result.push_str(raw);
            continue;
        }
        let comment = line
            .char_indices()
            .find_map(|(index, c)| {
                if c == '#' && index > 0 && line[..index].ends_with(char::is_whitespace) {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(line.len());
        let declaration = line[..comment].trim();
        if declaration.contains(char::is_whitespace) || declaration.contains('[') || declaration.contains(']') {
            return Err(DependencyError::Configuration(format!(
                "not a simple Conan requirement; refusing sync: {declaration}"
            )));
        }
        let (name, _, _, _, _) = reference_parts(declaration)?;
        if baseline.internal(&name) {
            result.push_str(raw);
            continue;
        }
        let canonical = baseline
            .direct
            .get(&name)
            .ok_or_else(|| DependencyError::Gate(format!("unregistered external direct requirement {declaration}")))?;
        let start = line
            .find(declaration)
            .required_configuration("ambiguous text requirement")?;
        result.push_str(&line[..start]);
        result.push_str(canonical);
        result.push_str(&line[start + declaration.len()..]);
        result.push_str(ending);
    }
    Ok(result)
}
fn copy_project(root: &Path, directory: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            continue;
        }
        if ty.is_dir() && directory == root && matches!(name.to_str(), Some(".git" | "build" | "target")) {
            continue;
        }
        let relative = entry.path().strip_prefix(root).configuration()?.to_path_buf();
        if ty.is_dir()
            && [".infra/.tmp", ".infra/transactions", ".infra/.transactions"]
                .iter()
                .any(|directory| relative == Path::new(directory))
        {
            continue;
        }
        let target = destination.join(relative);
        if ty.is_dir() {
            fs::create_dir_all(&target)?;
            copy_project(root, &entry.path(), destination)?;
        } else if ty.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
