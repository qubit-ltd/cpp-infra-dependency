// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Validate report-only source exclusions before any output can replace an
//! input.
use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;

use crate::dependency_error::DependencyError;
use crate::digest::input_path;
use crate::digest::project_path;
use crate::option_configuration::OptionConfiguration;
use crate::result_configuration::ResultConfiguration;

type Result<T> = std::result::Result<T, DependencyError>;

/// Resolves external output ancestors before deciding whether project input
/// rules apply.
pub(crate) fn report_output(project: &Path, path: &Path) -> Result<PathBuf> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
        || path.to_string_lossy().contains('\\')
    {
        return Err(DependencyError::Configuration(
            "report output cannot contain parent or backslash aliases".into(),
        ));
    }
    let output = resolve_location(path)?;
    if output.extension().and_then(|s| s.to_str()) != Some("json") {
        return Err(DependencyError::Configuration(
            "report output must be a JSON file".into(),
        ));
    }
    if output.starts_with(project) && !ordinary_build_output(project, &output) {
        let relative = relative_name(project, &output)?;
        validate_project_report(project, &relative)?;
    } else {
        validate_existing(&output)?;
    }
    Ok(output)
}

/// Merges validated inherited and current report paths in canonical byte order.
pub(crate) fn collect(project: &Path, output: &Path, context: Option<&Value>) -> Result<Vec<String>> {
    let mut paths = BTreeSet::new();
    if let Some(raw) = context.and_then(|value| value.get("source_excluded_paths")) {
        let inherited: Vec<String> = serde_json::from_value(raw.clone())
            .configuration_context("source_excluded_paths must be an array of file paths")?;
        for relative in inherited {
            validate_project_report(project, &relative)?;
            paths.insert(relative);
        }
    }
    if output.starts_with(project) && !ordinary_build_output(project, output) {
        let relative = relative_name(project, output)?;
        validate_project_report(project, &relative)?;
        paths.insert(relative);
    }
    Ok(paths.into_iter().collect())
}
/// Revalidates exclusions on fresh input checks, independently of config
/// hashing.
pub(crate) fn validate_paths(project: &Path, paths: &[String]) -> Result<()> {
    for relative in paths {
        validate_project_report(project, relative)?;
    }
    Ok(())
}
fn resolve_location(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    let mut existing = normalized.as_path();
    while !existing.exists() {
        if fs::symlink_metadata(existing).is_ok() {
            return Err(DependencyError::Configuration(
                "path contains a dangling symlink".into(),
            ));
        }
        existing = existing.parent().required_configuration("invalid output/input path")?;
    }
    let suffix = normalized.strip_prefix(existing).configuration()?;
    let canonical = existing.canonicalize()?;
    Ok(if suffix.as_os_str().is_empty() {
        canonical
    } else {
        canonical.join(suffix)
    })
}
fn relative_name(project: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(project)
        .configuration()?
        .to_str()
        .required_configuration("non-UTF8 report path")?
        .replace('\\', "/"))
}
fn ordinary_build_output(project: &Path, path: &Path) -> bool {
    path.strip_prefix(project)
        .ok()
        .and_then(|p| p.components().next())
        .is_some_and(|c| matches!(c.as_os_str().to_str(), Some("build" | "target")))
}
fn validate_project_report(project: &Path, relative: &str) -> Result<()> {
    let path = Path::new(relative);
    let original_components = path
        .components()
        .map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>();
    let normalized = original_components.map(|parts| parts.join("/"));
    if normalized.as_deref() != Some(relative)
        || relative.contains(':')
        || relative.is_empty()
        || relative.contains('\\')
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path.is_absolute()
        || path.components().any(|c| !matches!(c, Component::Normal(_)))
        || path.extension().and_then(|s| s.to_str()) != Some("json")
    {
        return Err(DependencyError::Configuration(format!(
            "source exclusion must be a normalized relative JSON file path: {relative}"
        )));
    }
    let absolute = project_path(project, relative)?;
    if absolute != project.join(relative) {
        return Err(DependencyError::Configuration(format!(
            "source exclusion cannot use symlink aliases: {relative}"
        )));
    }
    let first = path.components().next().and_then(|c| c.as_os_str().to_str());
    let lower = relative.to_ascii_lowercase();
    if matches!(
        first,
        Some(".git" | ".infra" | "conan" | "src" | "include" | "test" | "tests" | "generated" | "third_party")
    ) || lower.starts_with("readme")
        || lower.starts_with("cmake")
        || lower.starts_with("conanfile")
        || lower == "compile_commands.json"
    {
        return Err(DependencyError::Configuration(format!(
            "source exclusion is a reserved project input: {relative}"
        )));
    }
    if !ordinary_build_output(project, &absolute) {
        validate_declared_inputs(project, &absolute)?;
    }
    validate_existing(&absolute)
}
/// Rejects output artifacts that overlap declared project input roles.
pub(crate) fn validate_declared_inputs(project: &Path, output: &Path) -> Result<()> {
    let project_config = project.join(".infra/project.toml");
    if project_config.is_file() {
        let config = read_toml(project, &project_config)?;
        for key in ["source_roots", "test_roots", "generated_roots", "third_party_roots"] {
            if let Some(roots) = config.get(key).and_then(toml::Value::as_array) {
                for root in roots {
                    if let Some(root) = root.as_str() {
                        let root = project_path(project, root)?;
                        if output.starts_with(root) {
                            return Err(DependencyError::Configuration(
                                "report/source exclusion overlaps a configured project input root".into(),
                            ));
                        }
                    }
                }
            }
        }
    }
    let dependency_config = project.join(".infra/dependency/config.toml");
    if dependency_config.is_file() {
        let config = read_toml(project, &dependency_config)?;
        for key in ["recipe", "policy"] {
            reject_declared_file(project, output, config.get(key).and_then(toml::Value::as_str))?;
        }
        if let Some(contexts) = config.get("contexts").and_then(toml::Value::as_table) {
            for context in contexts.values() {
                for key in ["host_profile", "build_profile", "lockfile"] {
                    reject_declared_file(project, output, context.get(key).and_then(toml::Value::as_str))?;
                    if key != "lockfile"
                        && let Some(profile) = context.get(key).and_then(toml::Value::as_str)
                    {
                        reject_profile_input(project, output, &project.join(profile), &mut BTreeSet::new())?;
                    }
                }
            }
        }
    }
    let mut visited = BTreeSet::new();
    reject_preset_input(project, output, &project.join("CMakePresets.json"), &mut visited)?;
    Ok(())
}
fn reject_declared_file(project: &Path, output: &Path, relative: Option<&str>) -> Result<()> {
    if let Some(relative) = relative
        && project_path(project, relative)? == output
    {
        return Err(DependencyError::Configuration(
            "report/source exclusion overlaps a declared dependency input".into(),
        ));
    }
    Ok(())
}
fn reject_preset_input(project: &Path, output: &Path, input: &Path, visited: &mut BTreeSet<PathBuf>) -> Result<()> {
    let original_input = input;
    let input = resolve_location(input)?;
    if input == output {
        return Err(DependencyError::Configuration(
            "report/source exclusion overlaps a CMake preset input".into(),
        ));
    }
    if !input.exists() || !input.starts_with(project) || !visited.insert(input.clone()) {
        return Ok(());
    }
    let input = input_path(project, original_input)?;
    let value: Value = serde_json::from_slice(&fs::read(&input)?).configuration_context("invalid preset JSON")?;
    if let Some(includes) = value.get("include").and_then(Value::as_array) {
        for include in includes {
            if let Some(include) = include.as_str() {
                let parent = input.parent().required_configuration("preset has no parent")?;
                reject_preset_input(project, output, &parent.join(include), visited)?;
            }
        }
    }
    Ok(())
}
fn reject_profile_input(project: &Path, output: &Path, input: &Path, visited: &mut BTreeSet<PathBuf>) -> Result<()> {
    let original_input = input;
    let input = resolve_location(input)?;
    if input == output {
        return Err(DependencyError::Configuration(
            "report/source exclusion overlaps a Conan profile include input".into(),
        ));
    }
    if !input.exists() || !input.starts_with(project) || !visited.insert(input.clone()) {
        return Ok(());
    }
    let input = input_path(project, original_input)?;
    for line in fs::read_to_string(&input)?.lines().map(str::trim) {
        if let Some(include) = line.strip_prefix("include(").and_then(|value| value.strip_suffix(')')) {
            let include = include.trim().trim_matches(['\'', '"']);
            let parent = input.parent().required_configuration("profile has no parent")?;
            reject_profile_input(project, output, &parent.join(include), visited)?;
        }
    }
    Ok(())
}
fn read_toml(project: &Path, path: &Path) -> Result<toml::Value> {
    let path = input_path(project, path)?;
    toml::from_str(&fs::read_to_string(&path)?).configuration_context(format_args!("{}", path.display()))
}
fn validate_existing(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if !path.is_file() {
        return Err(DependencyError::Configuration(
            "report/source exclusion must name a file, not a directory".into(),
        ));
    }
    let value: Value = serde_json::from_slice(&fs::read(path)?).configuration_message(format_args!(
        "refusing to replace/exclude a non-report file: {}",
        path.display()
    ))?;
    let known_tool = value["tool"]["name"].as_str().is_some_and(|name| {
        matches!(
            name,
            "cpp-infra-tools"
                | "cpp-infra-ci"
                | "cpp-infra-verify"
                | "cpp-infra-style"
                | "cpp-infra-analyze"
                | "cpp-infra-dependency"
                | "cpp-infra-coverage"
                | "cpp-infra-pages"
        )
    });
    let required = [
        "bundle_id",
        "run_id",
        "task_id",
        "project",
        "report_root",
        "source_revision",
        "source_digest",
        "config_digest",
        "status",
        "started_at",
        "duration_ms",
        "external_versions",
        "commands",
        "diagnostics",
        "artifacts",
    ];
    if value["schema"] != 1
        || !known_tool
        || value["tool"]["version"].as_str().is_none()
        || value["tool"].get("revision").is_none()
        || required.iter().any(|key| value.get(*key).is_none())
        || !value["commands"].is_array()
        || !value["diagnostics"].is_array()
        || !value["artifacts"].is_array()
    {
        return Err(DependencyError::Configuration(format!(
            "refusing to replace/exclude an unknown business JSON file: {}",
            path.display()
        )));
    }
    Ok(())
}
