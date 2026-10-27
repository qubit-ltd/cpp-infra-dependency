// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Canonical content digests and bounded project paths.
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use sha2::Digest;
use sha2::Sha256;

use crate::dependency_error::DependencyError;
use crate::option_configuration::OptionConfiguration;
use crate::result_configuration::ResultConfiguration;

type Result<T> = std::result::Result<T, DependencyError>;

pub(crate) fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

/// Resolves a project path without allowing parent components or symlink
/// escape.
pub(crate) fn project_path(project: &Path, relative: &str) -> Result<PathBuf> {
    let rel = Path::new(relative);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        return Err(DependencyError::Configuration(format!(
            "path must be relative and remain inside project: {relative}"
        )));
    }
    let path = project.join(rel);
    let mut existing = path.as_path();
    while !existing.exists() {
        if fs::symlink_metadata(existing).is_ok() {
            return Err(DependencyError::Configuration(format!(
                "dangling symlink in path: {}",
                existing.display()
            )));
        }
        existing = existing
            .parent()
            .required_configuration(format_args!("invalid path: {relative}"))?;
    }
    let canonical = existing.canonicalize()?;
    if !canonical.starts_with(project) {
        return Err(DependencyError::Configuration(format!(
            "path escapes project through symlink: {relative}"
        )));
    }
    let suffix = path.strip_prefix(existing).configuration()?;
    if suffix.as_os_str().is_empty() {
        Ok(canonical)
    } else {
        Ok(canonical.join(suffix))
    }
}

/// Requires explicit consumed inputs to be regular, project-contained digest
/// inputs.
pub(crate) fn input_path(project: &Path, input: &Path) -> Result<PathBuf> {
    checked_input(project, input, true)
}

/// Allows a missing lock input while rejecting symlinks and digest-excluded
/// locations.
pub(crate) fn input_location(project: &Path, input: &Path) -> Result<PathBuf> {
    checked_input(project, input, false)
}

fn checked_input(project: &Path, input: &Path, require_exists: bool) -> Result<PathBuf> {
    let relative = if input.is_absolute() {
        input
            .strip_prefix(project)
            .configuration_message(format_args!("input escapes project: {}", input.display()))?
    } else {
        input
    };
    let mut current = project.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(name) => {
                current.push(name);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(DependencyError::Configuration(format!(
                            "input symlinks are not covered by the content digest: {}",
                            current.display()
                        )));
                    }
                    Ok(_) => {}
                    Err(error) if !require_exists && error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(DependencyError::Configuration(format!(
                            "input {}: {error}",
                            current.display()
                        )));
                    }
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
                if !current.starts_with(project) {
                    return Err(DependencyError::Configuration("input escapes project".into()));
                }
            }
            _ => {
                return Err(DependencyError::Configuration("invalid project input path".into()));
            }
        }
    }
    let relative = current.strip_prefix(project).configuration()?;
    let name = relative
        .to_str()
        .required_configuration("non-UTF8 input")?
        .replace('\\', "/");
    let first = relative.components().next().and_then(|c| c.as_os_str().to_str());
    let excluded = matches!(first, Some(".git" | "build" | "target"))
        || [".infra/.tmp", ".infra/transactions", ".infra/.transactions"]
            .iter()
            .any(|prefix| name == *prefix || name.starts_with(&format!("{prefix}/")));
    if excluded {
        return Err(DependencyError::Configuration(format!(
            "input is excluded from the project digest: {name}"
        )));
    }
    if (require_exists || current.exists()) && !current.is_file() {
        return Err(DependencyError::Configuration(format!(
            "input is not a regular file: {name}"
        )));
    }
    Ok(current)
}

pub(crate) fn project_absolute(project: &Path, path: &Path) -> Result<PathBuf> {
    let rel = if path.is_absolute() {
        path.strip_prefix(project)
            .configuration_message(format_args!("path is outside project: {}", path.display()))?
    } else {
        path
    };
    project_path(project, rel.to_str().required_configuration("path is not UTF-8")?)
}

pub(crate) fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    collect(root, root, &mut paths)?;
    paths.sort_by(|a, b| a.as_os_str().as_encoded_bytes().cmp(b.as_os_str().as_encoded_bytes()));
    Ok(paths)
}
fn collect(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            return Err(DependencyError::Configuration(format!(
                "generated output contains a symlink: {}",
                entry.path().display()
            )));
        }
        if ty.is_dir() {
            collect(root, &entry.path(), paths)?;
        } else if ty.is_file() {
            paths.push(entry.path().strip_prefix(root).configuration()?.to_path_buf());
        }
    }
    Ok(())
}

/// Computes the shared digest algorithm without following symbolic links.
pub(crate) fn project_digests(
    project: &Path,
    excluded: Option<&Path>,
    source_excluded_paths: &[String],
) -> Result<(String, String)> {
    crate::source_exclusions::validate_paths(project, source_excluded_paths)?;
    let mut paths = Vec::new();
    collect_project(project, project, excluded, &mut paths)?;
    let mut ordered = paths
        .into_iter()
        .map(|path| {
            let name = path
                .to_str()
                .required_configuration("non-UTF8 project path")?
                .replace('\\', "/");
            Ok((name, path))
        })
        .collect::<Result<Vec<_>>>()?;
    ordered.sort_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes()));
    let mut source = Sha256::new();
    let mut config = Sha256::new();
    for (name, path) in ordered {
        let first = path
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .unwrap_or("");
        let is_config = first == ".infra"
            || first == "conan"
            || matches!(
                name.as_str(),
                "CMakeLists.txt" | "CMakePresets.json" | "conanfile.py" | "conanfile.txt"
            );
        let bytes = fs::read(project.join(&path))?;
        if first != ".infra" && !source_excluded_paths.iter().any(|excluded| excluded == &name) {
            feed(&mut source, &name, &bytes);
        }
        if is_config {
            feed(&mut config, &name, &bytes);
        }
    }
    Ok((
        source
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        config
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    ))
}
fn collect_project(root: &Path, directory: &Path, excluded: Option<&Path>, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_symlink() || excluded.is_some_and(|p| path.starts_with(p)) {
            continue;
        }
        let name = entry.file_name();
        if ty.is_dir() {
            let relative = path.strip_prefix(root).configuration()?;
            let root_excluded = directory == root && matches!(name.to_str(), Some(".git" | "build" | "target"));
            let normalized = relative.to_string_lossy().replace('\\', "/");
            let transaction_excluded = matches!(
                Some(normalized.as_str()),
                Some(".infra/.tmp" | ".infra/transactions" | ".infra/.transactions")
            );
            if root_excluded || transaction_excluded {
                continue;
            }
            collect_project(root, &path, excluded, paths)?;
        } else if ty.is_file() {
            paths.push(path.strip_prefix(root).configuration()?.to_path_buf());
        }
    }
    Ok(())
}
fn feed(digest: &mut Sha256, path: &str, bytes: &[u8]) {
    digest.update((path.len() as u64).to_le_bytes());
    digest.update(path.as_bytes());
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}
