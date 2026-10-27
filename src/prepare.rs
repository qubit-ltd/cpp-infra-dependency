// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Install checked dependencies under the shared binary-directory lock.
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use fs2::FileExt;
use serde_json::Value;
use serde_json::json;

use crate::baseline::Baseline;
use crate::conan_adapter::arguments;
use crate::conan_adapter::save_graph;
use crate::conan_adapter::validate_lock;
use crate::config::Config;
use crate::dependency_error::DependencyError;
use crate::digest::files;
use crate::digest::project_absolute;
use crate::digest::project_digests;
use crate::digest::project_path;
use crate::digest::sha256;
use crate::graph::Graph;
use crate::option_configuration::OptionConfiguration;
use crate::project_lock::acquire;
use crate::report::Report;
use crate::report::atomic_json;
use crate::result_configuration::ResultConfiguration;
use crate::runner::conan_json;

type Result<T> = std::result::Result<T, DependencyError>;

pub(crate) fn install(
    project: &Path,
    config: &Config,
    baseline: &Baseline,
    name: &str,
    offline: bool,
    output: &Path,
    report: &mut Report,
) -> Result<()> {
    let fixed_output = context_output(project, config, name, output, report)?;
    let artifact_path = report.work.join("dependency-context.json");
    let fixed_artifact = context_output(project, config, name, &artifact_path, report)?;
    config.validate_toolchain(project, name)?;
    let context = config.context(name)?;
    let binary = config.binary_dir(project, name)?;
    let generators = project_path(project, &context.generators_dir)?;
    let recipe = project_path(project, &config.recipe)?;
    let lock = project_path(project, &context.lockfile)?;
    fs::create_dir_all(&binary)?;
    let lock_path = project_absolute(project, &binary.join(".cpp-infra.lock"))?;
    let guard = acquire(&lock_path, config.timeout_seconds)?;
    verify_marker(project, &binary)?;
    let (source, config_digest) = project_digests(project, Some(&report.root), &report.source_excluded_paths)?;
    if report.value["source_digest"] != source || report.value["config_digest"] != config_digest {
        return Err(DependencyError::Configuration(
            "project inputs changed while waiting for dependency lock".into(),
        ));
    }
    let build_state = binary.join(".cpp-infra-build-state.json");
    if build_state.is_file() {
        let mut state: Value =
            serde_json::from_slice(&fs::read(&build_state)?).configuration_context("invalid build state")?;
        if !state.is_object() {
            return Err(DependencyError::Configuration("build state must be an object".into()));
        }
        state["fingerprint"] = Value::Null;
        state["status"] = json!("dependency-updating");
        atomic_json(&build_state, &state)?;
    }
    let before_presets = fs::read(project.join("CMakePresets.json"))?;
    let before_user = fs::read(project.join("CMakeUserPresets.json")).ok();
    let mut args = vec!["install".into()];
    args.extend(arguments(project, config, context, offline, &recipe, Some(&lock))?);
    args.extend([
        "--format=json".into(),
        "--output-folder".into(),
        generators.to_string_lossy().into_owned(),
        if context.build_missing {
            "--build=missing".into()
        } else {
            "--build=never".into()
        },
    ]);
    let raw = conan_json(report, project, &args, config.timeout_seconds)?;
    let graph = Graph::parse(&raw)?;
    baseline.evaluate(&graph)?;
    validate_lock(&lock, &graph)?;
    if fs::read(project.join("CMakePresets.json"))? != before_presets
        || fs::read(project.join("CMakeUserPresets.json")).ok() != before_user
    {
        return Err(DependencyError::Configuration(
            "Conan changed project CMake presets; disable recipe user_presets_path generation".into(),
        ));
    }
    let actual = graph
        .generators_folder
        .as_ref()
        .required_configuration("Conan install JSON has no generators_folder")?;
    let actual = project_absolute(project, Path::new(actual))?.canonicalize()?;
    let expected = if generators.exists() {
        generators.canonicalize()?
    } else {
        generators.clone()
    };
    if actual != expected {
        return Err(DependencyError::Configuration(format!(
            "Conan layout generated files in {}, expected {}; adjust recipe folders.generators or context generators_dir",
            actual.display(),
            generators.display()
        )));
    }
    if !graph.generators.iter().any(|g| g == "CMakeToolchain")
        || !graph.generators.iter().any(|g| g == "CMakeDeps")
        || !generators.join("conan_toolchain.cmake").is_file()
    {
        return Err(DependencyError::Configuration(
            "recipe must generate CMakeDeps and CMakeToolchain in generators_dir".into(),
        ));
    }
    let (after_source, after_config) = project_digests(project, Some(&report.root), &report.source_excluded_paths)?;
    if after_source != source || after_config != config_digest {
        return Err(DependencyError::Configuration(
            "project inputs changed during Conan install".into(),
        ));
    }
    save_graph(report, &graph, &raw)?;
    let mut generated = Vec::new();
    for relative in files(&generators)? {
        let path = project_absolute(project, &generators.join(relative))?;
        generated.push(json!({"path":path,"sha256":sha256(&fs::read(&path)?)}));
    }
    let raw_bytes = serde_json::to_vec(&raw).configuration()?;
    let mut result = json!({"schema":1,"project":project,"context":name,"profile":context.profile,"source_digest":source,"source_excluded_paths":report.source_excluded_paths,"config_digest":config_digest,"baseline_id":baseline.baseline_id,"lock_digest":sha256(&fs::read(&lock)?),"graph_digest":sha256(&raw_bytes),"generators_dir":generators,"generated_files":generated,"settings":graph.root_settings,"options":graph.root_options,"toolchain":generators.join("conan_toolchain.cmake"),"binary_dir":binary,"build_profile_digest":sha256(&fs::read(project_path(project,&context.build_profile)?)?),"host_profile_digest":sha256(&fs::read(project_path(project,&context.host_profile)?)?),"recipe_digest":sha256(&fs::read(recipe)?),"fingerprint":null});
    result["fingerprint"] = json!(sha256(&serde_json::to_vec(&result).configuration()?));
    if context_output(project, config, name, output, report)? != fixed_output
        || context_output(project, config, name, &artifact_path, report)? != fixed_artifact
    {
        return Err(DependencyError::Configuration(
            "dependency context output identity changed during install".into(),
        ));
    }
    atomic_json(&fixed_output, &result)?;
    if fixed_output != fixed_artifact {
        atomic_json(&fixed_artifact, &result)?;
    }
    report.artifact("dependency-context", &fixed_artifact, true)?;
    report.value["data"]["dependency_context"] = result;
    FileExt::unlock(&guard)?;
    Ok(())
}
/// Allows context artifacts only in project build caches without replacing
/// inputs.
fn context_output(project: &Path, config: &Config, name: &str, requested: &Path, report: &Report) -> Result<PathBuf> {
    let output = project_absolute(project, requested)?;
    let relative = output.strip_prefix(project).configuration()?;
    let in_cache = relative
        .components()
        .next()
        .is_some_and(|part| matches!(part.as_os_str().to_str(), Some("build" | "target")));
    let context = config.context(name)?;
    let generators = project_path(project, &context.generators_dir)?;
    let reserved_name = output.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
        name.starts_with(".cpp-infra-")
            || matches!(
                name,
                "CMakePresets.json" | "CMakeUserPresets.json" | "compile_commands.json"
            )
    });
    if !in_cache
        || output.extension().and_then(|extension| extension.to_str()) != Some("json")
        || output.starts_with(&generators)
        || output == report.path
        || reserved_name
    {
        return Err(DependencyError::Configuration("dependency context output must be a separate JSON artifact under build/ or target/, outside generators and project inputs".into()));
    }
    crate::source_exclusions::validate_declared_inputs(project, &output)?;
    if output.exists() {
        if !output.is_file() {
            return Err(DependencyError::Configuration(
                "dependency context output is not a regular file".into(),
            ));
        }
        let value: Value = serde_json::from_slice(&fs::read(&output)?)
            .configuration_message("refusing to replace a non-context business file")?;
        let fields = [
            "source_digest",
            "config_digest",
            "baseline_id",
            "lock_digest",
            "graph_digest",
            "generators_dir",
            "fingerprint",
        ];
        if value["schema"] != 1
            || value["project"].as_str() != project.to_str()
            || value["context"] != name
            || value["profile"] != context.profile
            || !value["generated_files"].is_array()
            || fields.iter().any(|field| value[*field].as_str().is_none())
        {
            return Err(DependencyError::Configuration(
                "refusing to replace an unknown business file with a dependency context".into(),
            ));
        }
    }
    Ok(output)
}
fn verify_marker(project: &Path, binary: &Path) -> Result<()> {
    let marker = binary.join(".cpp-infra-project.json");
    if marker.exists() {
        let value: Value =
            serde_json::from_slice(&fs::read(&marker)?).configuration_context("invalid build management marker")?;
        if value["project"].as_str() != project.to_str() || value["schema"] != 1 {
            return Err(DependencyError::Configuration(
                "build directory belongs to another project".into(),
            ));
        }
    } else {
        let entries = fs::read_dir(binary)?.collect::<std::result::Result<Vec<_>, _>>()?;
        if entries.iter().any(|entry| entry.file_name() != ".cpp-infra.lock") {
            return Err(DependencyError::Configuration(
                "refusing to take over unmarked nonempty binary directory".into(),
            ));
        }
        atomic_json(&marker, &json!({"schema":1,"project":project}))?;
    }
    let cache = binary.join("CMakeCache.txt");
    if cache.is_file() {
        let text = fs::read_to_string(cache)?;
        if let Some(line) = text
            .lines()
            .find(|line| line.starts_with("CMAKE_HOME_DIRECTORY:INTERNAL="))
        {
            let value = line.trim_start_matches("CMAKE_HOME_DIRECTORY:INTERNAL=");
            if Path::new(value).canonicalize()? != project {
                return Err(DependencyError::Configuration(
                    "CMake cache source root differs from project".into(),
                ));
            }
        }
    }
    Ok(())
}
