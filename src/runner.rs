// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Shell-free commands with bounded execution, cancellation and retained logs.
use std::fs;
use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use serde_json::Value;
use serde_json::json;

use crate::dependency_error::DependencyError;
use crate::option_configuration::OptionConfiguration;
use crate::report::Report;
use crate::report::atomic_write;
use crate::result_configuration::ResultConfiguration;

static CANCELLED: AtomicBool = AtomicBool::new(false);
type Result<T> = std::result::Result<T, DependencyError>;
pub(crate) fn is_cancelled() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}
pub(crate) fn install_cancel_handler() -> Result<()> {
    ctrlc::set_handler(|| CANCELLED.store(true, Ordering::SeqCst))
        .configuration_context("cannot install cancellation handler")
}
pub(crate) fn conan(report: &mut Report, project: &Path, args: &[String], timeout: u64) -> Result<Vec<u8>> {
    fs::create_dir_all(&report.work)?;
    let index = report.value["commands"].as_array().map_or(0, Vec::len);
    let stdout_path = report.work.join(format!("command-{index}.stdout"));
    let stderr_path = report.work.join(format!("command-{index}.stderr"));
    let log_path = report.work.join(format!("command-{index}.log"));
    let log_name = log_path
        .strip_prefix(&report.root)
        .configuration()?
        .to_string_lossy()
        .replace('\\', "/");
    let started = Instant::now();
    let mut command = Command::new("conan");
    command
        .args(args)
        .current_dir(project)
        .stdin(Stdio::null())
        .stdout(File::create(&stdout_path)?)
        .stderr(File::create(&stderr_path)?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            if let Some(commands) = report.value["commands"].as_array_mut() {
                commands.push(json!({"program":"conan","args":args,"cwd":project,"env":recorded_environment(),"exit_code":null,"duration_ms":started.elapsed().as_millis(),"log":log_name}));
            }
            atomic_write(&log_path, error.to_string().as_bytes())?;
            return Err(DependencyError::Infrastructure(error));
        }
    };
    let mut cancelled = false;
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if CANCELLED.load(Ordering::SeqCst) || started.elapsed() > Duration::from_secs(timeout) {
            cancelled = CANCELLED.load(Ordering::SeqCst);
            timed_out = !cancelled;
            #[cfg(unix)]
            {
                // The child owns this process group, so cancellation includes recipe
                // subprocesses.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
            }
            #[cfg(not(unix))]
            child.kill()?;
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(25));
    };
    let stdout = fs::read(&stdout_path)?;
    let stderr = fs::read(&stderr_path)?;
    let log = format!(
        "stdout:\n{}\nstderr:\n{}",
        redact(&String::from_utf8_lossy(&stdout)),
        redact(&String::from_utf8_lossy(&stderr))
    );
    atomic_write(&log_path, log.as_bytes())?;
    if let Some(commands) = report.value["commands"].as_array_mut() {
        commands.push(json!({"program":"conan","args":args,"cwd":project,"env":recorded_environment(),"exit_code":status.code(),"duration_ms":started.elapsed().as_millis(),"log":log_name}));
    }
    // These are disposable captures; only the redacted retained log is a report
    // artifact.
    fs::remove_file(stdout_path)?;
    fs::remove_file(stderr_path)?;
    report.artifact("command-log", &log_path, false)?;
    if cancelled {
        return Err(DependencyError::Cancelled);
    }
    if timed_out {
        return Err(DependencyError::Gate(format!(
            "Conan command timed out after {timeout}s (see {log_name})"
        )));
    }
    if !status.success() {
        return Err(DependencyError::Gate(format!(
            "Conan command failed with {:?}: {} (see {log_name})",
            status.code(),
            redact(&String::from_utf8_lossy(&stderr))
        )));
    }
    Ok(stdout)
}
pub(crate) fn conan_json(report: &mut Report, project: &Path, args: &[String], timeout: u64) -> Result<Value> {
    let bytes = conan(report, project, args, timeout)?;
    serde_json::from_slice(&bytes).configuration_context("unsupported Conan JSON output")
}
pub(crate) fn verify_version(report: &mut Report, project: &Path, timeout: u64) -> Result<()> {
    let bytes = conan(report, project, &["--version".into()], timeout)?;
    let text = String::from_utf8_lossy(&bytes);
    let version = text
        .trim()
        .strip_prefix("Conan version ")
        .required_configuration(format_args!("unsupported Conan version output: {text}"))?;
    let parts: Vec<_> = version.split('.').collect();
    let minor = parts.get(1).and_then(|p| p.parse::<u32>().ok());
    if parts.len() != 3
        || parts[0] != "2"
        || !minor.is_some_and(|m| (28..=33).contains(&m))
        || parts[2].parse::<u32>().is_err()
    {
        return Err(DependencyError::Configuration(format!(
            "unsupported Conan version {version}; supported adapter: 2.28.x through 2.33.x"
        )));
    }
    report.value["external_versions"]["conan"] = json!({"actual":version,"constraint":">=2.28.0,<2.34.0"});
    Ok(())
}
fn recorded_environment() -> Value {
    let mut result = serde_json::Map::new();
    for (name, value) in std::env::vars() {
        if name == "PATH" || name.starts_with("CONAN_") {
            let sensitive = ["PASSWORD", "TOKEN", "SECRET", "KEY", "USERNAME"]
                .iter()
                .any(|word| name.contains(word));
            result.insert(name, json!(if sensitive { "[REDACTED]".to_string() } else { value }));
        }
    }
    Value::Object(result)
}
fn redact(text: &str) -> String {
    let mut result = text.to_string();
    let mut offset = 0;
    while let Some(start) = result[offset..].find("://") {
        let begin = offset + start + 3;
        let end = result[begin..]
            .find(['/', ' ', '\n', '\r', '\t', '"', '\''])
            .map_or(result.len(), |n| begin + n);
        if let Some(at) = result[begin..end].rfind('@') {
            result.replace_range(begin..begin + at, "[REDACTED]");
            offset = begin + "[REDACTED]".len() + 1;
        } else {
            offset = end;
        }
        if offset >= result.len() {
            break;
        }
    }
    result
}
