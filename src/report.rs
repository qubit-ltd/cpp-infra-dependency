// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Atomic common reports and content-addressed artifacts.
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use chrono::Utc;
use serde_json::Value;
use serde_json::json;
use tempfile::NamedTempFile;

use crate::dependency_error::DependencyError;
use crate::digest::sha256;
use crate::option_configuration::OptionConfiguration;
use crate::result_configuration::ResultConfiguration;

type Result<T> = std::result::Result<T, DependencyError>;
pub(crate) struct Report {
    pub value: Value,
    pub root: PathBuf,
    pub work: PathBuf,
    pub call_id: String,
    pub path: PathBuf,
    pub source_excluded_paths: Vec<String>,
    pub started: Instant,
}
impl Report {
    pub(crate) fn new(project: &Path, path: PathBuf, root: PathBuf, run_id: &str) -> Self {
        Self {
            value: json!({"schema":1,"tool":{"name":"cpp-infra-dependency","version":env!("CARGO_PKG_VERSION"),"revision":option_env!("CPP_INFRA_BUILD_REVISION")},"bundle_id":"development","run_id":run_id,"task_id":"dependency","project":project,"report_root":root,"source_revision":null,"source_digest":null,"config_digest":null,"status":"error","started_at":Utc::now().to_rfc3339(),"duration_ms":0,"external_versions":{},"commands":[],"diagnostics":[],"artifacts":[],"data":{}}),
            work: root.join("dependency").join(run_id),
            call_id: run_id.to_string(),
            root,
            path,
            source_excluded_paths: Vec::new(),
            started: Instant::now(),
        }
    }
    pub(crate) fn artifact(&mut self, kind: &str, path: &Path, required: bool) -> Result<()> {
        let path = path.canonicalize()?;
        let root = self.root.canonicalize()?;
        let relative = path
            .strip_prefix(root)
            .configuration_message(format_args!("artifact outside report_root: {}", path.display()))?;
        let entry = json!({"kind":kind,"path":relative,"sha256":sha256(&fs::read(&path)?),"required":required});
        let artifacts = self.value["artifacts"]
            .as_array_mut()
            .required_configuration("internal artifacts list")?;
        if let Some(previous) = artifacts
            .iter_mut()
            .find(|artifact| artifact["kind"] == kind && artifact["path"] == entry["path"])
        {
            *previous = entry;
        } else {
            artifacts.push(entry);
        }
        Ok(())
    }
    pub(crate) fn diagnostic(&mut self, code: &str, message: &str, severity: &str) {
        if let Some(diagnostics) = self.value["diagnostics"].as_array_mut() {
            diagnostics.push(json!({"code":code,"severity":severity,"message":message,"project_relative_path":null,"line":null,"column":null,"rule":null,"suppression_reason":null}));
        }
    }
    pub(crate) fn finish(&mut self, result: &Result<()>) -> Result<()> {
        self.value["duration_ms"] = json!(self.started.elapsed().as_millis());
        self.value["status"] = json!(match result {
            Ok(()) => "passed",
            Err(DependencyError::Gate(_)) => "failed",
            Err(DependencyError::Cancelled) => "cancelled",
            Err(_) => "error",
        });
        if let Err(error) = result {
            self.diagnostic(
                match error {
                    DependencyError::Gate(_) => "dependency-gate",
                    DependencyError::Configuration(_) => "dependency-configuration",
                    DependencyError::Infrastructure(_) => "dependency-infrastructure",
                    DependencyError::Cancelled => "dependency-cancelled",
                },
                &error.to_string(),
                "error",
            );
        }
        let project = self.value["project"]
            .as_str()
            .required_configuration("report project identity is missing")?;
        let safe_output = crate::source_exclusions::report_output(Path::new(project), &self.path)?;
        if safe_output != self.path {
            return Err(DependencyError::Configuration(
                "report output path changed during execution".into(),
            ));
        }
        atomic_json(&self.path, &self.value)
    }
}
pub(crate) fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).configuration()?;
    atomic_write(path, &bytes)
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().required_configuration("output has no parent directory")?;
    fs::create_dir_all(parent)?;
    let permissions = fs::metadata(path).ok().map(|m| m.permissions());
    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    if let Some(permissions) = permissions {
        temp.as_file().set_permissions(permissions)?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|e| DependencyError::Infrastructure(e.error))?;
    Ok(())
}
