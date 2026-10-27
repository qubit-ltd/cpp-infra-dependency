// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Common command entry, run-context validation and dependency workflows.
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use chrono::Utc;
use clap::Parser;
use clap::Subcommand;
use serde_json::Value;
use serde_json::json;

use crate::conan_adapter::arguments;
use crate::conan_adapter::graph_info;
use crate::conan_adapter::require_locks;
use crate::conan_adapter::save_graph;
use crate::conan_adapter::validate_lock;
use crate::conan_adapter::verify_remotes;
use crate::config::Config;
use crate::dependency_error::DependencyError;
use crate::digest::project_absolute;
use crate::digest::project_digests;
use crate::digest::project_path;
use crate::digest::sha256;
use crate::lock_update;
use crate::prepare;
use crate::report::Report;
use crate::report::atomic_json;
use crate::runner::install_cancel_handler;
use crate::runner::verify_version;
use crate::source_exclusions;
use crate::text_sync;

static NEXT_RUN: AtomicU64 = AtomicU64::new(0);
use crate::option_configuration::OptionConfiguration;
use crate::result_configuration::ResultConfiguration;

type Result<T> = std::result::Result<T, DependencyError>;
#[derive(Parser)]
#[command(
    name = "cpp-infra-dependency",
    about = "Conan 2 dependency inventory, policy, installation and explicit updates",
    disable_version_flag = true
)]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    project: PathBuf,
    #[arg(long, global = true)]
    report: Option<PathBuf>,
    #[arg(long, global = true)]
    run_context: Option<PathBuf>,
    #[arg(long, global = true)]
    offline: bool,
    #[arg(long, global = true)]
    json: bool,
    #[arg(long)]
    version: bool,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Print commands without executing Conan or accessing remote services.
    Plan {
        #[arg(long)]
        context: String,
    },
    /// Resolve and inventory host/build/test requirements without installation.
    Inventory {
        #[arg(long)]
        context: String,
    },
    /// Enforce direct and resolved policy against the locked dependency graph.
    Check {
        #[arg(long)]
        context: String,
    },
    /// Check policy, install locked dependencies, and export a verified
    /// context.
    Prepare {
        #[arg(long)]
        context: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Update simple conanfile.txt requirements after staging validation.
    Sync {
        #[arg(long)]
        dry_run: bool,
    },
    /// Explicit dependency lock operations.
    Lock {
        #[command(subcommand)]
        command: LockCommand,
    },
}
#[derive(Subcommand)]
enum LockCommand {
    Update {
        #[arg(long)]
        context: String,
        #[arg(long)]
        dry_run: bool,
    },
}

pub(crate) fn entry() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };
    if cli.version {
        if cli.json {
            println!(
                "{}",
                json!({"name":"cpp-infra-dependency","version":env!("CARGO_PKG_VERSION"),"revision":option_env!("CPP_INFRA_BUILD_REVISION"),"protocol":1})
            );
        } else {
            println!("cpp-infra-dependency {}", env!("CARGO_PKG_VERSION"));
        }
        return 0;
    }
    match start(&cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            error.exit_code()
        }
    }
}
fn start(cli: &Cli) -> Result<i32> {
    let project = cli.project.canonicalize()?;
    if !project.is_dir() {
        return Err(DependencyError::Configuration("--project must be a directory".into()));
    }
    let run_id = format!(
        "{}-{}-{}",
        Utc::now().format("%Y%m%dT%H%M%S%3fZ"),
        std::process::id(),
        NEXT_RUN.fetch_add(1, Ordering::SeqCst)
    );
    let default_root = project_absolute(&project, &project.join("build/infra-runs").join(&run_id))?;
    let path = match &cli.report {
        Some(path) => {
            if path.is_absolute() {
                path.clone()
            } else {
                project.join(path)
            }
        }
        None => default_root.join("dependency-report.json"),
    };
    let path = if path.starts_with(&project) {
        project_absolute(&project, &path)?
    } else {
        path
    };
    let path = source_exclusions::report_output(&project, &path)?;
    let context = cli
        .run_context
        .as_ref()
        .map(|path| {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                project.join(path)
            };
            let bytes = fs::read(path)?;
            serde_json::from_slice::<Value>(&bytes).configuration_context("invalid run context JSON")
        })
        .transpose()?;
    let excluded = source_exclusions::collect(&project, &path, context.as_ref())?;
    let mut report = Report::new(&project, path, default_root, &run_id);
    report.value["source_excluded_paths"] = json!(excluded);
    report.source_excluded_paths = excluded;

    let result = execute(cli, &project, &mut report);
    if let Err(error) = &result {
        eprintln!("{error}");
    }
    if let Err(error) = report.finish(&result) {
        eprintln!("cannot write dependency report: {error}");
        return Ok(error.exit_code());
    }
    Ok(result.as_ref().err().map_or(0, DependencyError::exit_code))
}
fn execute(cli: &Cli, project: &Path, report: &mut Report) -> Result<()> {
    install_cancel_handler()?;
    let (source, config_digest) = project_digests(project, Some(&report.root), &report.source_excluded_paths)?;
    report.value["source_digest"] = json!(source);
    report.value["config_digest"] = json!(config_digest);
    if !matches!(cli.command, Some(Command::Plan { .. }))
        && let Ok(output) = ProcessCommand::new("git")
            .args(["rev-parse", "--verify", "HEAD"])
            .current_dir(project)
            .output()
        && output.status.success()
    {
        report.value["source_revision"] = json!(String::from_utf8_lossy(&output.stdout).trim());
    }
    if let Some(path) = &cli.run_context {
        validate_context(project, path, report)?;
    }
    let (config, baseline) = Config::load(project)?;
    report.value["data"]["baseline_id"] = json!(baseline.baseline_id);
    report.value["data"]["offline_boundary"] = json!(
        "--offline disables Conan remotes; selected Python recipes are not network sandboxed. Conan may populate its shared local cache."
    );
    let command = cli
        .command
        .as_ref()
        .required_configuration("a subcommand is required; see --help")?;
    if let Command::Plan { context } = command {
        validate_selected_profile(cli, &config.context(context)?.profile)?;
        return plan(project, &config, context, cli.offline, report, cli.json);
    }
    verify_version(report, project, config.timeout_seconds)?;
    match command {
        Command::Inventory { context } | Command::Check { context } | Command::Prepare { context, .. } => {
            require_locks(project, &config)?;
            let selected = config.context(context)?;
            validate_selected_profile(cli, selected.profile.as_str())?;
            verify_remotes(project, &config, selected, &baseline, cli.offline, report)?;
            let lock = project_path(project, &selected.lockfile)?;
            let (graph, raw) = graph_info(
                project,
                &config,
                selected,
                cli.offline,
                &project_path(project, &config.recipe)?,
                &lock,
                report,
            )?;
            save_graph(report, &graph, &raw)?;
            baseline.evaluate(&graph)?;
            validate_lock(&lock, &graph)?;
            if let Command::Prepare { output, .. } = command {
                let output = output
                    .clone()
                    .unwrap_or_else(|| report.work.join("dependency-context.json"));
                prepare::install(project, &config, &baseline, context, cli.offline, &output, report)?;
            }
            ensure_unchanged(project, report)?;
            if matches!(command, Command::Inventory { .. }) {
                println!("{}", report.value["data"]["inventory"]);
            }
        }
        Command::Sync { dry_run } => {
            for context in config.contexts.values() {
                verify_remotes(project, &config, context, &baseline, cli.offline, report)?;
            }
            text_sync::sync(project, &config, &baseline, cli.offline, *dry_run, report)?;
            refresh_digests(project, report)?;
            println!("{}", report.value["data"]["sync"]);
        }
        Command::Lock {
            command: LockCommand::Update { context, dry_run },
        } => {
            let selected = config.context(context)?;
            validate_selected_profile(cli, selected.profile.as_str())?;
            verify_remotes(project, &config, selected, &baseline, cli.offline, report)?;
            lock_update::update(project, &config, &baseline, context, cli.offline, *dry_run, report)?;
            refresh_digests(project, report)?;
            println!("{}", report.value["data"]["lock_update"]);
        }
        Command::Plan { .. } => {}
    }
    Ok(())
}
fn ensure_unchanged(project: &Path, report: &Report) -> Result<()> {
    let (source, config) = project_digests(project, Some(&report.root), &report.source_excluded_paths)?;
    if report.value["source_digest"] != source || report.value["config_digest"] != config {
        return Err(DependencyError::Configuration(
            "project inputs changed while evaluating Conan graph".into(),
        ));
    }
    Ok(())
}
fn refresh_digests(project: &Path, report: &mut Report) -> Result<()> {
    let (source, config) = project_digests(project, Some(&report.root), &report.source_excluded_paths)?;
    report.value["source_digest"] = json!(source);
    report.value["config_digest"] = json!(config);
    Ok(())
}
fn validate_selected_profile(cli: &Cli, profile: &str) -> Result<()> {
    if let Some(path) = &cli.run_context {
        let path = if path.is_absolute() {
            path.clone()
        } else {
            cli.project.join(path)
        };
        let value: Value =
            serde_json::from_slice(&fs::read(path)?).configuration_context("invalid run context JSON")?;
        if value["profile"].as_str() != Some(profile) {
            return Err(DependencyError::Configuration(
                "run context profile differs from Conan context profile".into(),
            ));
        }
    }
    Ok(())
}
fn validate_context(project: &Path, path: &Path, report: &mut Report) -> Result<()> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project.join(path)
    };
    let value: Value = serde_json::from_slice(&fs::read(path)?).configuration_context("invalid run context JSON")?;
    if value["schema"] != 1
        || value["project"].as_str() != project.to_str()
        || value["source_digest"] != report.value["source_digest"]
        || value["config_digest"] != report.value["config_digest"]
    {
        return Err(DependencyError::Configuration(
            "stale or invalid run context project/source/config digest".into(),
        ));
    }
    for name in ["run_id", "task_id", "bundle_id", "report_root"] {
        if value[name].as_str().is_none_or(str::is_empty) {
            return Err(DependencyError::Configuration(format!("run context missing {name}")));
        }
    }
    match value.get("profile") {
        Some(Value::Null) => {}
        Some(Value::String(profile)) if !profile.is_empty() => {}
        _ => return Err(DependencyError::Configuration("run context missing profile".into())),
    }
    let root = project_absolute(
        project,
        Path::new(
            value["report_root"]
                .as_str()
                .required_configuration("missing report root")?,
        ),
    )?;
    if !root.starts_with(project.join("build")) {
        return Err(DependencyError::Configuration(
            "run report_root must be within project build directory".into(),
        ));
    }
    let bundle = value["bundle_id"]
        .as_str()
        .required_configuration("missing bundle id")?;
    match (value.get("runtime_snapshot"), value.get("runtime_snapshot_digest")) {
        (Some(snapshot), Some(digest)) => {
            let snapshot = snapshot
                .as_str()
                .required_configuration("runtime_snapshot must be an absolute path")?;
            if !Path::new(snapshot).is_absolute() {
                return Err(DependencyError::Configuration(
                    "runtime_snapshot must be absolute".into(),
                ));
            }
            let bytes = fs::read(snapshot)?;
            if digest.as_str() != Some(sha256(&bytes).as_str()) {
                return Err(DependencyError::Configuration(
                    "runtime snapshot digest mismatch".into(),
                ));
            }
            let snapshot: Value = serde_json::from_slice(&bytes).configuration_context("invalid runtime snapshot")?;
            if snapshot["schema"] != 1 || snapshot["bundle_id"] != bundle {
                return Err(DependencyError::Configuration(
                    "runtime snapshot schema/bundle mismatch".into(),
                ));
            }
            let tools = snapshot["tools"]
                .as_array()
                .required_configuration("runtime snapshot missing tools")?;
            let identities: Vec<_> = tools.iter().filter(|t| t["name"] == "cpp-infra-dependency").collect();
            if identities.len() != 1 {
                return Err(DependencyError::Configuration(
                    "runtime snapshot must contain one dependency tool identity".into(),
                ));
            }
            let identity = identities[0];
            let executable = std::env::current_exe()?.canonicalize()?;
            let declared = identity["binary"]
                .as_str()
                .required_configuration("runtime tool missing binary")?;
            if !Path::new(declared).is_absolute()
                || sha256(&fs::read(Path::new(declared))?) != sha256(&fs::read(&executable)?)
                || identity["protocol"] != 1
                || identity["version"] != env!("CARGO_PKG_VERSION")
                || identity["revision"] != json!(option_env!("CPP_INFRA_BUILD_REVISION"))
                || identity["executable_sha256"].as_str() != Some(sha256(&fs::read(&executable)?).as_str())
            {
                return Err(DependencyError::Configuration(
                    "runtime dependency executable identity mismatch".into(),
                ));
            }
            report.value["data"]["runtime_snapshot"] =
                json!({"bundle_id":bundle,"offline":snapshot["offline"],"stale":snapshot["stale"]});
        }
        (None, None) => {}
        _ => {
            return Err(DependencyError::Configuration(
                "runtime_snapshot and runtime_snapshot_digest must be supplied together".into(),
            ));
        }
    }
    report.work = root.join("dependency").join(&report.call_id);
    report.root = root;
    report.value["report_root"] = json!(report.root);
    for name in ["run_id", "task_id", "bundle_id"] {
        report.value[name] = value[name].clone();
    }
    Ok(())
}
fn plan(
    project: &Path,
    config: &Config,
    name: &str,
    offline: bool,
    report: &mut Report,
    json_output: bool,
) -> Result<()> {
    let context = config.context(name)?;
    let recipe = project_path(project, &config.recipe)?;
    let lock = project_path(project, &context.lockfile)?;
    let mut args = vec!["graph".into(), "info".into()];
    args.extend(arguments(project, config, context, offline, &recipe, Some(&lock))?);
    args.push("--format=json".into());
    let output = json!({"schema":1,"nodes":[{"id":"dependency-graph","tool":"conan","args":args,"profile":context.profile,"needs":[],"resource_locks":[],"timeout_seconds":config.timeout_seconds,"required":true,"output":"runtime graph discovery"}]});
    report.value["data"]["plan"] = output.clone();
    let artifact = report.work.join("plan.json");
    atomic_json(&artifact, &output)?;
    report.artifact("dependency-plan", &artifact, true)?;
    let _ = json_output;
    println!("{output}");
    Ok(())
}
