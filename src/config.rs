// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Strict project configuration and Conan profile bindings.
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use crate::baseline::Baseline;
use crate::dependency_error::DependencyError;
use crate::digest::input_location;
use crate::digest::input_path;
use crate::digest::project_absolute;
use crate::digest::project_path;
use crate::option_configuration::OptionConfiguration;
use crate::result_configuration::ResultConfiguration;

type Result<T> = std::result::Result<T, DependencyError>;
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    pub schema: u32,
    pub backend: String,
    pub recipe: String,
    pub policy: String,
    pub contexts: BTreeMap<String, Context>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}
fn default_timeout() -> u64 {
    300
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Context {
    pub build_profile: String,
    pub host_profile: String,
    pub lockfile: String,
    pub profile: String,
    pub generators_dir: String,
    #[serde(default)]
    pub build_missing: bool,
    #[serde(default)]
    pub remotes: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Project {
    schema: u32,
    kind: String,
    #[serde(default)]
    source_roots: Vec<String>,
    #[serde(default)]
    test_roots: Vec<String>,
    #[serde(default)]
    generated_roots: Vec<String>,
    #[serde(default)]
    third_party_roots: Vec<String>,
    profiles: BTreeMap<String, Profile>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    configure: String,
    build: String,
    test: Option<String>,
    conan_context: Option<String>,
}

pub(crate) fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path)?;
    toml::from_str(&text).configuration_context(format_args!("{}", path.display()))
}
impl Config {
    pub(crate) fn load(project: &Path) -> Result<(Self, Baseline)> {
        let path = input_path(project, &project.join(".infra/dependency/config.toml"))?;
        if !path.is_file() {
            return Err(DependencyError::Configuration(
                "dependency config is missing: .infra/dependency/config.toml".into(),
            ));
        }
        let config: Self = read_toml(&path)?;
        if config.schema != 1 || config.backend != "conan2" || config.contexts.is_empty() || config.timeout_seconds == 0
        {
            return Err(DependencyError::Configuration(
                "expected schema=1, backend=conan2, nonempty contexts and positive timeout".into(),
            ));
        }
        let recipe = input_path(project, &project.join(&config.recipe))?;
        if !recipe.is_file()
            || !matches!(
                recipe.file_name().and_then(|x| x.to_str()),
                Some("conanfile.txt" | "conanfile.py")
            )
        {
            return Err(DependencyError::Configuration(
                "recipe must be an existing conanfile.txt or conanfile.py".into(),
            ));
        }
        let policy: Baseline = read_toml(&input_path(project, &project.join(&config.policy))?)?;
        policy.validate()?;
        let p: Project = read_toml(&input_path(project, &project.join(".infra/project.toml"))?)?;
        if p.schema != 1 || !matches!(p.kind.as_str(), "library" | "application" | "header-only") {
            return Err(DependencyError::Configuration("unsupported project schema/kind".into()));
        }
        let mut roots = Vec::new();
        for name in p
            .source_roots
            .iter()
            .chain(&p.test_roots)
            .chain(&p.generated_roots)
            .chain(&p.third_party_roots)
        {
            let path = project_path(project, name)?;
            if !path.is_dir() {
                return Err(DependencyError::Configuration(format!(
                    "configured root is not a directory: {name}"
                )));
            }
            let path = path.canonicalize()?;
            if roots
                .iter()
                .any(|other: &PathBuf| path.starts_with(other) || other.starts_with(&path))
            {
                return Err(DependencyError::Configuration(format!(
                    "overlapping project roots: {name}"
                )));
            }
            roots.push(path);
        }
        for (name, c) in &config.contexts {
            if name.is_empty()
                || matches!(name.as_str(), "." | "..")
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            {
                return Err(DependencyError::Configuration("empty context identifier".into()));
            }
            let profile = p
                .profiles
                .get(&c.profile)
                .required_configuration(format_args!("unknown project profile: {}", c.profile))?;
            if profile.conan_context.as_deref() != Some(name)
                || profile.build.is_empty()
                || profile.test.as_ref().is_some_and(String::is_empty)
            {
                return Err(DependencyError::Configuration(format!(
                    "profile {} must bind conan_context={name}",
                    c.profile
                )));
            }
            let binary = binary_dir(project, &profile.configure)?;
            let generators = project_path(project, &c.generators_dir)?;
            if generators == binary || !generators.starts_with(&binary) {
                return Err(DependencyError::Configuration(format!(
                    "generators_dir must be within {}",
                    binary.display()
                )));
            }
            input_location(project, &project.join(&c.lockfile))?;
            validate_profile(project, &project.join(&c.build_profile), &mut BTreeSet::new())?;
            validate_profile(project, &project.join(&c.host_profile), &mut BTreeSet::new())?;
            let mut remotes = BTreeSet::new();
            for remote in &c.remotes {
                if !policy.remotes.contains_key(remote) || !remotes.insert(remote) {
                    return Err(DependencyError::Configuration(format!(
                        "remote is not uniquely registered by policy: {remote}"
                    )));
                }
            }
        }
        for (name, profile) in &p.profiles {
            if let Some(context) = &profile.conan_context
                && config.contexts.get(context).is_none_or(|c| &c.profile != name)
            {
                return Err(DependencyError::Configuration(format!(
                    "project profile {name} has inconsistent Conan context"
                )));
            }
        }
        Ok((config, policy))
    }
    pub(crate) fn context(&self, name: &str) -> Result<&Context> {
        self.contexts
            .get(name)
            .required_configuration(format_args!("unknown Conan context: {name}"))
    }
    pub(crate) fn validate_toolchain(&self, project: &Path, name: &str) -> Result<()> {
        let context = self.context(name)?;
        let p: Project = read_toml(&input_path(project, &project.join(".infra/project.toml"))?)?;
        let preset_name = &p.profiles[&context.profile].configure;
        let mut presets = BTreeMap::new();
        load_presets(
            project,
            &project.join("CMakePresets.json"),
            &mut BTreeSet::new(),
            &mut presets,
        )?;
        let preset = resolve_preset(preset_name, &presets, &mut BTreeSet::new())?;
        let variable = &preset["cacheVariables"]["CMAKE_TOOLCHAIN_FILE"];
        let path = preset["toolchainFile"]
            .as_str()
            .or_else(|| variable.as_str())
            .or_else(|| variable["value"].as_str())
            .required_configuration("selected configure preset must explicitly bind the Conan toolchain")?;
        let expanded = path
            .replace(
                "${sourceDir}",
                project.to_str().required_configuration("non-UTF8 project")?,
            )
            .replace("${presetName}", preset_name);
        if expanded.contains('$') {
            return Err(DependencyError::Configuration(
                "unsupported toolchain path macro".into(),
            ));
        }
        let expected = project_path(project, &context.generators_dir)?.join("conan_toolchain.cmake");
        let path = Path::new(&expanded);
        let from_source = project_absolute(project, path)?;
        let from_binary = if path.is_absolute() {
            from_source.clone()
        } else {
            project_absolute(project, &self.binary_dir(project, name)?.join(path))?
        };
        if expected != from_source && expected != from_binary {
            return Err(DependencyError::Configuration(format!(
                "configure preset toolchain must be {}",
                expected.display()
            )));
        }
        Ok(())
    }
    pub(crate) fn binary_dir(&self, project: &Path, name: &str) -> Result<PathBuf> {
        let c = self.context(name)?;
        let p: Project = read_toml(&input_path(project, &project.join(".infra/project.toml"))?)?;
        binary_dir(project, &p.profiles[&c.profile].configure)
    }
}
fn validate_profile(project: &Path, path: &Path, active: &mut BTreeSet<PathBuf>) -> Result<()> {
    let path = input_path(project, path)?;
    if !path.starts_with(project) {
        return Err(DependencyError::Configuration(format!(
            "profile escapes project: {}",
            path.display()
        )));
    }
    if !active.insert(path.clone()) {
        return Err(DependencyError::Configuration("Conan profile include cycle".into()));
    }
    let text = fs::read_to_string(&path)?;
    for line in text.lines().map(str::trim) {
        if let Some(include) = line.strip_prefix("include(").and_then(|x| x.strip_suffix(')')) {
            let name = include.trim().trim_matches(['\'', '"']);
            if !name.starts_with("./") && !name.starts_with("../") && !Path::new(name).is_absolute() {
                return Err(DependencyError::Configuration("profile include must explicitly use ./, ../ or a project-contained absolute path; bare includes may resolve from Conan home".into()));
            }
            let parent = path.parent().required_configuration("profile missing parent")?;
            let included = parent.join(name);
            validate_profile(project, &included, active)?;
        } else if line.starts_with("include") || line.contains("{{") || line.contains("{%") {
            return Err(DependencyError::Configuration(format!(
                "dynamic or malformed profile includes are unsupported: {}",
                path.display()
            )));
        }
    }
    active.remove(&path);
    Ok(())
}

/// Resolves the supported, project-contained Presets include/inheritance
/// subset.
fn binary_dir(project: &Path, name: &str) -> Result<PathBuf> {
    let mut presets = BTreeMap::new();
    load_presets(
        project,
        &project.join("CMakePresets.json"),
        &mut BTreeSet::new(),
        &mut presets,
    )?;
    let preset = resolve_preset(name, &presets, &mut BTreeSet::new())?;
    let template = preset
        .get("binaryDir")
        .and_then(Value::as_str)
        .required_configuration(format_args!("preset {name} has no binaryDir"))?;
    let expanded = template
        .replace(
            "${sourceDir}",
            project.to_str().required_configuration("non-UTF8 project")?,
        )
        .replace("${presetName}", name);
    if expanded.contains('$') {
        return Err(DependencyError::Configuration("unsupported binaryDir macro".into()));
    }
    let path = project_absolute(project, Path::new(&expanded))?;
    let base = project_path(project, "build/infra")?;
    if base != project.join("build/infra") || path == base || !path.starts_with(base) {
        return Err(DependencyError::Configuration(
            "binaryDir must be under build/infra".into(),
        ));
    }
    Ok(path)
}
fn load_presets(
    project: &Path,
    path: &Path,
    active: &mut BTreeSet<PathBuf>,
    presets: &mut BTreeMap<String, Value>,
) -> Result<()> {
    let path = input_path(project, path)?;
    if !path.starts_with(project) || !active.insert(path.clone()) {
        return Err(DependencyError::Configuration(
            "preset include escapes project or is cyclic".into(),
        ));
    }
    let value: Value = serde_json::from_slice(&fs::read(&path)?).configuration_context("preset JSON")?;
    if value["version"].as_u64() != Some(6) {
        return Err(DependencyError::Configuration(
            "only CMake Presets schema 6 is supported".into(),
        ));
    }
    if let Some(includes) = value.get("include") {
        for include in includes
            .as_array()
            .required_configuration("preset include must be array")?
        {
            let include = include
                .as_str()
                .required_configuration("preset include must be string")?;
            let parent = path.parent().required_configuration("preset has no parent")?;
            load_presets(project, &parent.join(include), active, presets)?;
        }
    }
    if let Some(configures) = value.get("configurePresets") {
        for preset in configures
            .as_array()
            .required_configuration("configurePresets must be array")?
        {
            let name = preset["name"].as_str().required_configuration("preset missing name")?;
            if presets.insert(name.into(), preset.clone()).is_some() {
                return Err(DependencyError::Configuration(format!(
                    "duplicate configure preset: {name}"
                )));
            }
        }
    }
    active.remove(&path);
    Ok(())
}
fn resolve_preset(name: &str, presets: &BTreeMap<String, Value>, active: &mut BTreeSet<String>) -> Result<Value> {
    if !active.insert(name.into()) {
        return Err(DependencyError::Configuration("preset inheritance cycle".into()));
    }
    let preset = presets
        .get(name)
        .required_configuration(format_args!("unknown configure preset: {name}"))?;
    let mut result = serde_json::Map::new();
    if let Some(inherits) = preset.get("inherits") {
        let names = if let Some(name) = inherits.as_str() {
            vec![name]
        } else {
            inherits
                .as_array()
                .required_configuration("invalid preset inherits")?
                .iter()
                .map(|v| v.as_str().required_configuration("invalid inherited preset name"))
                .collect::<Result<Vec<_>>>()?
        };
        for parent in names.into_iter().rev() {
            if let Value::Object(fields) = resolve_preset(parent, presets, active)? {
                merge_preset_fields(&mut result, &fields);
            }
        }
    }
    merge_preset_fields(
        &mut result,
        preset.as_object().required_configuration("preset must be object")?,
    );
    active.remove(name);
    Ok(Value::Object(result))
}

fn merge_preset_fields(target: &mut serde_json::Map<String, Value>, fields: &serde_json::Map<String, Value>) {
    for (key, value) in fields {
        if matches!(key.as_str(), "cacheVariables" | "environment")
            && let Some(existing) = target.get_mut(key).and_then(Value::as_object_mut)
            && let Some(incoming) = value.as_object()
        {
            existing.extend(incoming.clone());
        } else {
            target.insert(key.clone(), value.clone());
        }
    }
}
