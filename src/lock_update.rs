// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Explicit lock creation with policy validation before atomic replacement.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use fs2::FileExt;
use serde_json::Value;
use serde_json::json;
use tempfile::tempdir_in;

use crate::baseline::Baseline;
use crate::conan_adapter::arguments;
use crate::conan_adapter::graph_info;
use crate::conan_adapter::save_graph;
use crate::conan_adapter::validate_lock;
use crate::config::Config;
use crate::dependency_error::DependencyError;
use crate::digest::project_digests;
use crate::digest::project_path;
use crate::digest::sha256;
use crate::option_configuration::OptionConfiguration;
use crate::project_lock::acquire;
use crate::report::Report;
use crate::report::atomic_write;
use crate::result_configuration::ResultConfiguration;
use crate::runner::conan;

type Result<T> = std::result::Result<T, DependencyError>;
pub(crate) fn update(
    project: &Path,
    config: &Config,
    baseline: &Baseline,
    name: &str,
    offline: bool,
    dry_run: bool,
    report: &mut Report,
) -> Result<()> {
    let context = config.context(name)?;
    let recipe = project_path(project, &config.recipe)?;
    let lock = project_path(project, &context.lockfile)?;
    let old = match fs::read(&lock) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    fs::create_dir_all(&report.work)?;
    let temporary = tempdir_in(&report.work)?;
    let candidate = temporary.path().join("conan.lock");
    let mut args = vec!["lock".into(), "create".into()];
    args.extend(arguments(project, config, context, offline, &recipe, None)?);
    args.extend(["--lockfile-out".into(), candidate.to_string_lossy().into_owned()]);
    if !offline {
        args.push("--update".into());
    }
    conan(report, project, &args, config.timeout_seconds)?;
    let (graph, raw) = graph_info(project, config, context, offline, &recipe, &candidate, report)?;
    baseline.evaluate(&graph)?;
    validate_lock(&candidate, &graph)?;
    let bytes = fs::read(&candidate)?;
    let before = old.as_deref().map(lock_references).transpose()?.unwrap_or_default();
    let after = lock_references(&bytes)?;
    report.value["data"]["lock_update"] = json!({"context":name,"dry_run":dry_run,"added":after.difference(&before).collect::<Vec<_>>(),"removed":before.difference(&after).collect::<Vec<_>>(),"requires_verification":true});
    save_graph(report, &graph, &raw)?;
    let artifact = report.work.join(format!("{name}-candidate.lock"));
    atomic_write(&artifact, &bytes)?;
    report.artifact("dependency-lock-candidate", &artifact, true)?;
    if !dry_run {
        let guard_path = project_path(
            project,
            &format!(
                "build/infra/locks/dependency-lock-{}.lock",
                sha256(context.lockfile.as_bytes())
            ),
        )?;
        let guard = acquire(&guard_path, config.timeout_seconds)?;
        let (source, config_digest) = project_digests(project, Some(&report.root), &report.source_excluded_paths)?;
        if report.value["source_digest"] != source || report.value["config_digest"] != config_digest {
            return Err(DependencyError::Configuration(
                "project inputs changed during lock validation; old lock preserved".into(),
            ));
        }
        let current = match fs::read(&lock) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if current != old {
            return Err(DependencyError::Configuration(
                "lock changed concurrently; old lock preserved".into(),
            ));
        }
        atomic_write(&lock, &bytes)?;
        FileExt::unlock(&guard)?;
    }
    report.diagnostic(
        "verification-required",
        "Lock update requires verify/ci validation before integration.",
        "warning",
    );
    Ok(())
}
fn lock_references(bytes: &[u8]) -> Result<BTreeSet<String>> {
    let value: Value = serde_json::from_slice(bytes).configuration_context("invalid lock JSON")?;
    let mut refs = BTreeSet::new();
    for category in ["requires", "build_requires", "python_requires", "config_requires"] {
        if let Some(entries) = value[category].as_array() {
            for entry in entries {
                let reference = entry.as_str().required_configuration("non-string lock reference")?;
                refs.insert(format!("{category}:{reference}"));
            }
        }
    }
    Ok(refs)
}
