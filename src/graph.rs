// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Conan 2 JSON graph adapter preserving reference and context identity.
use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::dependency_error::DependencyError;
use crate::option_configuration::OptionConfiguration;

type Result<T> = std::result::Result<T, DependencyError>;
#[derive(Clone, Serialize)]
pub(crate) struct Node {
    pub id: String,
    pub name: String,
    pub version: String,
    pub user: Option<String>,
    pub channel: Option<String>,
    pub reference: String,
    pub recipe_revision: Option<String>,
    pub package_id: Option<String>,
    pub package_revision: Option<String>,
    pub context: String,
    pub binary: Option<String>,
    pub settings: Value,
    pub options: Value,
    pub python_requires: Value,
}
#[derive(Clone, Serialize)]
pub(crate) struct Requirement {
    pub node_id: String,
    pub name: String,
    pub requirement: String,
    pub reference: String,
    pub context: String,
    pub category: String,
}
#[derive(Clone, Serialize)]
pub(crate) struct Graph {
    pub nodes: Vec<Node>,
    pub direct: Vec<Requirement>,
    pub edges: BTreeMap<String, Value>,
    pub root_settings: Value,
    pub root_options: Value,
    pub generators_folder: Option<String>,
    pub generators: Vec<String>,
}

type ReferenceParts = (String, String, Option<String>, Option<String>, Option<String>);

pub(crate) fn reference_parts(reference: &str) -> Result<ReferenceParts> {
    let (base, revision) = match reference.split_once('#') {
        Some((a, b)) => (a, Some(b.split('%').next().unwrap_or(b).to_string())),
        None => (reference, None),
    };
    let (nv, uc) = match base.split_once('@') {
        Some((a, b)) => (a, Some(b)),
        None => (base, None),
    };
    let (name, version) = nv
        .split_once('/')
        .required_configuration(format_args!("invalid Conan reference: {reference}"))?;
    if name.is_empty() || version.is_empty() {
        return Err(DependencyError::Configuration(format!(
            "invalid Conan reference: {reference}"
        )));
    }
    let (user, channel) = if let Some(uc) = uc {
        let (u, c) = uc
            .split_once('/')
            .required_configuration(format_args!("invalid user/channel: {reference}"))?;
        (Some(u.into()), Some(c.into()))
    } else {
        (None, None)
    };
    Ok((name.into(), version.into(), user, channel, revision))
}
impl Graph {
    pub(crate) fn parse(value: &Value) -> Result<Self> {
        let graph = value
            .get("graph")
            .and_then(Value::as_object)
            .required_configuration("unsupported Conan graph JSON: missing graph object")?;
        if graph.get("error").is_some_and(|v| !v.is_null()) {
            return Err(DependencyError::Gate(format!(
                "Conan graph resolution error: {}",
                graph["error"]
            )));
        }
        let raw = graph
            .get("nodes")
            .and_then(Value::as_object)
            .required_configuration("unsupported Conan graph JSON: missing nodes")?;
        let root = raw.get("0").required_configuration("Conan graph has no root node 0")?;
        let mut nodes = Vec::new();
        let mut edges = BTreeMap::new();
        for (id, node) in raw {
            if !node.is_object()
                || !node.get("ref").is_some_and(|r| r.is_null() || r.is_string())
                || node.get("package_id").is_none()
            {
                return Err(DependencyError::Configuration(format!(
                    "unsupported Conan node {id}: missing ref/package_id"
                )));
            }
            let context = node["context"]
                .as_str()
                .filter(|c| matches!(*c, "host" | "build"))
                .required_configuration(format_args!("unsupported context on Conan node {id}"))?;
            let dependencies = node["dependencies"]
                .as_object()
                .required_configuration(format_args!("missing dependency edges on node {id}"))?;
            for (target, edge) in dependencies {
                if !raw.contains_key(target)
                    || edge["ref"].as_str().is_none()
                    || edge["require"].as_str().is_none()
                    || edge["direct"].as_bool().is_none()
                {
                    return Err(DependencyError::Configuration(format!(
                        "unsupported Conan dependency edge {id}->{target}"
                    )));
                }
            }
            edges.insert(id.clone(), node["dependencies"].clone());
            if id == "0" {
                continue;
            }
            let reference = node["ref"]
                .as_str()
                .required_configuration(format_args!("dependency node {id} missing reference"))?;
            let (name, version, user, channel, revision) = reference_parts(reference)?;
            nodes.push(Node {
                id: id.clone(),
                name,
                version,
                user,
                channel,
                reference: reference.into(),
                recipe_revision: revision.or_else(|| node["rrev"].as_str().map(str::to_owned)),
                package_id: node["package_id"].as_str().map(str::to_owned),
                package_revision: node["prev"].as_str().map(str::to_owned),
                context: context.into(),
                binary: node["binary"].as_str().map(str::to_owned),
                settings: node.get("settings").cloned().unwrap_or(Value::Null),
                options: node.get("options").cloned().unwrap_or(Value::Null),
                python_requires: node.get("python_requires").cloned().unwrap_or(Value::Null),
            });
        }
        let mut direct = Vec::new();
        for (id, edge) in root["dependencies"]
            .as_object()
            .required_configuration("missing root requirements")?
        {
            if edge["direct"].as_bool() != Some(true) {
                continue;
            }
            let requirement = edge["require"]
                .as_str()
                .required_configuration("missing original direct requirement")?;
            let (name, _, _, _, _) = reference_parts(requirement)?;
            let context = raw[id]["context"]
                .as_str()
                .required_configuration("missing direct node context")?;
            let category = if edge["build"].as_bool() == Some(true) {
                "tool_requires"
            } else if edge["test"].as_bool() == Some(true) {
                "test_requires"
            } else {
                "requires"
            };
            direct.push(Requirement {
                node_id: id.clone(),
                name,
                requirement: requirement.into(),
                reference: edge["ref"].as_str().required_configuration("missing edge ref")?.into(),
                context: context.into(),
                category: category.into(),
            });
        }
        let generators = root["generators"]
            .as_array()
            .required_configuration("missing root generators")?
            .iter()
            .map(|g| {
                g.as_str()
                    .map(str::to_owned)
                    .required_configuration("invalid generator entry")
            })
            .collect::<Result<Vec<_>>>()?;
        let settings = root
            .get("settings")
            .filter(|s| s.is_object())
            .required_configuration("missing Conan root settings")?;
        Ok(Self {
            nodes,
            direct,
            edges,
            root_settings: settings.clone(),
            root_options: root.get("options").cloned().unwrap_or(Value::Null),
            generators_folder: root["generators_folder"].as_str().map(str::to_owned),
            generators,
        })
    }
}
