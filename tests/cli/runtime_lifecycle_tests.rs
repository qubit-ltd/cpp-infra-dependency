// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use tempfile::tempdir;

use crate::Fixture;
use crate::assert_code;

fn adapter_replace(fixture: &Fixture, before: &str, after: &str) {
    let path = fixture.path().join("bin/conan");
    let original = fs::read_to_string(&path).expect("fixture Conan adapter");
    assert!(original.contains(before), "missing adapter replacement anchor");
    fs::write(path, original.replace(before, after)).expect("customized Conan adapter");
}

fn assert_diagnostic(report: &Value, code: &str, message: &str) {
    assert!(
        report["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|entry| {
                entry["code"] == code
                    && entry["severity"] == "error"
                    && entry["message"].as_str().expect("diagnostic message").contains(message)
            }),
        "missing {code}: {message} in {report}"
    );
}

fn assert_no_install(report: &Value) {
    assert!(
        report["commands"]
            .as_array()
            .expect("commands")
            .iter()
            .all(|command| { command["args"][0] != "install" })
    );
}

fn run_context(fixture: &Fixture) -> Value {
    assert_code(&fixture.run(&["plan", "--context", "linux-debug"]), 0);
    let report = fixture.report();
    json!({
        "schema": 1,
        "project": fixture.path().canonicalize().expect("project"),
        "source_digest": report["source_digest"],
        "config_digest": report["config_digest"],
        "run_id": "lifecycle-run",
        "task_id": "dependency-test",
        "bundle_id": "lifecycle-bundle",
        "profile": "debug",
        "report_root": fixture.path().join("build/infra-runs/lifecycle-run")
    })
}

fn write_context(fixture: &Fixture, value: &Value) {
    fs::write(fixture.path().join("build/run-context.json"), value.to_string()).expect("run context");
}

#[test]
fn test_cli_help_parse_errors_and_missing_subcommand_have_distinct_report_lifecycle() {
    for (args, code, expected) in [
        (vec!["--help"], 0, "Conan 2 dependency"),
        (vec!["unknown-command"], 2, "unrecognized subcommand"),
        (vec!["check"], 2, "--context"),
    ] {
        let fixture = Fixture::new();
        let output = fixture.run(&args);
        assert_code(&output, code);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains(expected), "{text}");
        assert!(!fixture.path().join("build/report.json").exists());
    }
    let fixture = Fixture::new();
    assert_code(&fixture.run(&[]), 2);
    let report = fixture.report();
    assert_eq!(report["status"], "error");
    assert_diagnostic(&report, "dependency-configuration", "a subcommand is required");
    assert_eq!(report["commands"], json!([]));
}

#[test]
fn test_invalid_run_context_json_is_rejected_before_creating_report() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path().join("build")).expect("build directory");
    fs::write(fixture.path().join("build/run-context.json"), "{invalid").expect("invalid run context");
    let output = fixture.run(&[
        "--run-context",
        "build/run-context.json",
        "check",
        "--context",
        "linux-debug",
    ]);
    assert_code(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid run context JSON"));
    assert!(!fixture.path().join("build/report.json").exists());
}

#[test]
fn test_conan_version_protocol_rejects_unsupported_shapes_before_graph_commands() {
    for (version, expected) in [
        ("Conan 2.28.0", "unsupported Conan version output"),
        ("Conan version 1.66.0", "supported adapter: 2.28.x through 2.33.x"),
        ("Conan version 2.27.9", "unsupported Conan version 2.27.9"),
        ("Conan version 2.34.0", "unsupported Conan version 2.34.0"),
        ("Conan version 2.28.dev", "unsupported Conan version 2.28.dev"),
        ("Conan version 2.28.0.1", "unsupported Conan version 2.28.0.1"),
    ] {
        let fixture = Fixture::new();
        adapter_replace(&fixture, "Conan version 2.28.0", version);
        assert_code(&fixture.run(&["check", "--context", "linux-debug"]), 2);
        let report = fixture.report();
        assert_diagnostic(&report, "dependency-configuration", expected);
        assert_eq!(report["commands"].as_array().expect("commands").len(), 1);
        assert_eq!(report["commands"][0]["args"], json!(["--version"]));
        assert_eq!(report["commands"][0]["exit_code"], 0);
        assert!(report["external_versions"]["conan"].is_null());
    }
}

#[test]
fn test_non_json_conan_response_records_successful_command_but_configuration_failure() {
    let fixture = Fixture::new();
    adapter_replace(&fixture, "print(json.dumps(g))", "print('not a JSON graph')");
    assert_code(&fixture.run(&["inventory", "--context", "linux-debug"]), 2);
    let report = fixture.report();
    assert_diagnostic(&report, "dependency-configuration", "unsupported Conan JSON output");
    let commands = report["commands"].as_array().expect("commands");
    assert_eq!(commands.last().expect("graph command")["exit_code"], 0);
    assert_eq!(commands.last().expect("graph command")["args"][0], "graph");
    assert!(
        report["artifacts"]
            .as_array()
            .expect("artifacts")
            .iter()
            .all(|artifact| artifact["kind"] == "command-log")
    );
    assert!(report["data"]["inventory"].is_null());
}

#[cfg(unix)]
#[test]
fn test_missing_conan_is_infrastructure_failure_with_launch_metadata_and_log() {
    let fixture = Fixture::new();
    let empty_bin = fixture.path().join("empty-bin");
    fs::create_dir(&empty_bin).expect("empty executable search directory");
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-dependency"))
        .arg("--project")
        .arg(fixture.path())
        .arg("--report")
        .arg(fixture.path().join("build/report.json"))
        .args(["check", "--context", "linux-debug"])
        .env("PATH", &empty_bin)
        .output()
        .expect("run without Conan on PATH");
    assert_code(&output, 3);
    let report = fixture.report();
    assert_eq!(report["status"], "error");
    assert_diagnostic(&report, "dependency-infrastructure", "No such file or directory");
    assert_eq!(report["commands"].as_array().expect("commands").len(), 1);
    let command = &report["commands"][0];
    assert_eq!(command["args"], json!(["--version"]));
    assert_eq!(command["cwd"], json!(fixture.path().canonicalize().expect("project")));
    assert_eq!(command["env"]["PATH"], json!(empty_bin));
    assert!(command["exit_code"].is_null());
    let log = PathBuf::from(report["report_root"].as_str().expect("report root"))
        .join(command["log"].as_str().expect("launch log"));
    assert!(
        fs::read_to_string(log)
            .expect("launch error log")
            .contains("No such file or directory")
    );
}

#[test]
fn test_failed_conan_redacts_url_credentials_and_sensitive_environment_in_retained_artifacts() {
    let fixture = Fixture::new();
    adapter_replace(
        &fixture,
        "fixture Conan failure",
        "failed https://alice:secret-one@example.test/a and ssh://bob:secret-two@host.test/b",
    );
    fs::write(fixture.path().join("fail-conan"), "fail").expect("failure switch");
    let output = Command::new(env!("CARGO_BIN_EXE_cpp-infra-dependency"))
        .arg("--project")
        .arg(fixture.path())
        .arg("--report")
        .arg(fixture.path().join("build/report.json"))
        .args(["check", "--context", "linux-debug"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                fixture.path().join("bin").display(),
                std::env::var("PATH").expect("PATH")
            ),
        )
        .env("CONAN_LOGIN_USERNAME", "alice")
        .env("CONAN_PASSWORD", "secret-one")
        .env("CONAN_TOKEN", "secret-two")
        .env("CONAN_HOME", fixture.path().join("build/conan-home"))
        .output()
        .expect("run credential failure");
    assert_code(&output, 1);
    let report = fixture.report();
    assert_eq!(report["status"], "failed");
    assert_diagnostic(&report, "dependency-gate", "https://[REDACTED]@example.test/a");
    let command = report["commands"]
        .as_array()
        .expect("commands")
        .last()
        .expect("failed command");
    assert_eq!(command["exit_code"], 1);
    for key in ["CONAN_LOGIN_USERNAME", "CONAN_PASSWORD", "CONAN_TOKEN"] {
        assert_eq!(command["env"][key], "[REDACTED]");
    }
    assert_eq!(
        command["env"]["CONAN_HOME"],
        json!(fixture.path().join("build/conan-home"))
    );
    let root = PathBuf::from(report["report_root"].as_str().expect("report root"));
    let artifact = report["artifacts"]
        .as_array()
        .expect("artifacts")
        .iter()
        .find(|artifact| artifact["path"] == command["log"])
        .expect("retained command log");
    assert_eq!(artifact["required"], false);
    let log_path = root.join(artifact["path"].as_str().expect("log path"));
    let log_bytes = fs::read(&log_path).expect("retained log bytes");
    assert_eq!(
        artifact["sha256"],
        Sha256::digest(&log_bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let log = String::from_utf8(log_bytes).expect("log UTF-8");
    assert!(log.contains("ssh://[REDACTED]@host.test/b"));
    let all_retained = format!("{report}{log}{}", String::from_utf8_lossy(&output.stderr));
    assert!(!all_retained.contains("secret-one"));
    assert!(!all_retained.contains("secret-two"));
    for entry in fs::read_dir(log_path.parent().expect("log directory")).expect("captures directory") {
        let path = entry.expect("capture entry").path();
        assert!(!matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("stdout" | "stderr")
        ));
    }
}

#[test]
fn test_run_context_requires_complete_identity_and_matching_selected_profile() {
    let fixture = Fixture::new();
    let valid = run_context(&fixture);
    for field in ["run_id", "task_id", "bundle_id", "profile", "report_root"] {
        let mut context = valid.clone();
        context[field] = json!("");
        write_context(&fixture, &context);
        assert_code(
            &fixture.run(&[
                "--run-context",
                "build/run-context.json",
                "check",
                "--context",
                "linux-debug",
            ]),
            2,
        );
        let report = fixture.report();
        assert_diagnostic(
            &report,
            "dependency-configuration",
            &format!("run context missing {field}"),
        );
        assert_eq!(report["commands"], json!([]));
    }
    let mut context = valid.clone();
    context["report_root"] = json!(fixture.path().join("target/run"));
    write_context(&fixture, &context);
    assert_code(
        &fixture.run(&[
            "--run-context",
            "build/run-context.json",
            "plan",
            "--context",
            "linux-debug",
        ]),
        2,
    );
    assert_diagnostic(
        &fixture.report(),
        "dependency-configuration",
        "run report_root must be within project build",
    );
    let mut context = valid;
    context["profile"] = json!("release");
    write_context(&fixture, &context);
    assert_code(
        &fixture.run(&[
            "--run-context",
            "build/run-context.json",
            "plan",
            "--context",
            "linux-debug",
        ]),
        2,
    );
    assert_diagnostic(
        &fixture.report(),
        "dependency-configuration",
        "run context profile differs",
    );
    assert_eq!(fixture.report()["commands"], json!([]));
}

#[test]
fn test_runtime_snapshot_structure_and_tool_identity_are_checked_before_conan() {
    let fixture = Fixture::new();
    let mut context = run_context(&fixture);
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_cpp-infra-dependency"))
        .canonicalize()
        .expect("binary");
    let executable_sha256 = Sha256::digest(fs::read(&binary).expect("binary bytes"))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let version: Value =
        serde_json::from_slice(&fixture.run(&["--version", "--json"]).stdout).expect("version identity");
    let identity = json!({"name":"cpp-infra-dependency", "binary":binary, "protocol":1,
        "version":version["version"], "revision":version["revision"], "executable_sha256":executable_sha256});
    let valid =
        json!({"schema":1, "bundle_id":"lifecycle-bundle", "tools":[identity.clone()], "offline":true, "stale":false});
    let snapshot_path = fixture.path().join("build/runtime-snapshot.json");
    context["runtime_snapshot"] = json!(snapshot_path);
    let mut cases = vec![(json!({"schema":2}), "runtime snapshot schema/bundle mismatch")];
    let mut snapshot = valid.clone();
    snapshot["bundle_id"] = json!("another-bundle");
    cases.push((snapshot, "runtime snapshot schema/bundle mismatch"));
    let mut snapshot = valid.clone();
    snapshot["tools"] = Value::Null;
    cases.push((snapshot, "runtime snapshot missing tools"));
    for tools in [json!([]), json!([identity.clone(), identity.clone()])] {
        let mut snapshot = valid.clone();
        snapshot["tools"] = tools;
        cases.push((snapshot, "runtime snapshot must contain one dependency tool identity"));
    }
    let mut snapshot = valid.clone();
    snapshot["tools"][0]
        .as_object_mut()
        .expect("tool identity")
        .remove("binary");
    cases.push((snapshot, "runtime tool missing binary"));
    for (field, value) in [
        ("binary", json!("relative-tool")),
        ("protocol", json!(2)),
        ("version", json!("999.0.0")),
        ("revision", json!("different-revision")),
        ("executable_sha256", json!("wrong-hash")),
        ("binary", json!(fixture.path().join("conanfile.txt"))),
    ] {
        let mut snapshot = valid.clone();
        snapshot["tools"][0][field] = value;
        cases.push((snapshot, "runtime dependency executable identity mismatch"));
    }
    for (snapshot, message) in cases {
        let bytes = serde_json::to_vec(&snapshot).expect("snapshot bytes");
        fs::write(&snapshot_path, &bytes).expect("snapshot file");
        context["runtime_snapshot_digest"] = json!(
            Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        write_context(&fixture, &context);
        assert_code(
            &fixture.run(&[
                "--run-context",
                "build/run-context.json",
                "check",
                "--context",
                "linux-debug",
            ]),
            2,
        );
        let report = fixture.report();
        assert_diagnostic(&report, "dependency-configuration", message);
        assert_eq!(report["commands"], json!([]));
    }
    let bytes = b"{invalid snapshot";
    fs::write(&snapshot_path, bytes).expect("invalid snapshot bytes");
    context["runtime_snapshot_digest"] = json!(
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    write_context(&fixture, &context);
    assert_code(
        &fixture.run(&[
            "--run-context",
            "build/run-context.json",
            "check",
            "--context",
            "linux-debug",
        ]),
        2,
    );
    assert_diagnostic(
        &fixture.report(),
        "dependency-configuration",
        "invalid runtime snapshot",
    );
}

#[test]
fn test_runtime_snapshot_requires_paired_fields_and_absolute_path() {
    let fixture = Fixture::new();
    let valid = run_context(&fixture);
    for field in ["runtime_snapshot", "runtime_snapshot_digest"] {
        let mut context = valid.clone();
        context[field] = json!("unpaired");
        write_context(&fixture, &context);
        assert_code(
            &fixture.run(&[
                "--run-context",
                "build/run-context.json",
                "check",
                "--context",
                "linux-debug",
            ]),
            2,
        );
        assert_diagnostic(
            &fixture.report(),
            "dependency-configuration",
            "must be supplied together",
        );
        assert_eq!(fixture.report()["commands"], json!([]));
    }
    for (path, message) in [
        (json!(42), "runtime_snapshot must be an absolute path"),
        (json!("build/runtime.json"), "runtime_snapshot must be absolute"),
    ] {
        let mut context = valid.clone();
        context["runtime_snapshot"] = path;
        context["runtime_snapshot_digest"] = json!("unused");
        write_context(&fixture, &context);
        assert_code(
            &fixture.run(&[
                "--run-context",
                "build/run-context.json",
                "check",
                "--context",
                "linux-debug",
            ]),
            2,
        );
        assert_diagnostic(&fixture.report(), "dependency-configuration", message);
        assert_eq!(fixture.report()["commands"], json!([]));
    }
}

#[test]
fn test_prepare_rejects_invalid_management_marker_without_changing_it_or_installing() {
    for (bytes, message) in [
        (b"{broken".as_slice(), "invalid build management marker"),
        (
            b"{\"schema\":1,\"project\":\"/another/project\"}".as_slice(),
            "build directory belongs to another project",
        ),
        (
            b"{\"schema\":2}".as_slice(),
            "build directory belongs to another project",
        ),
    ] {
        let fixture = Fixture::new();
        let binary = fixture.path().join("build/infra/infra-debug");
        fs::create_dir_all(&binary).expect("binary directory");
        let marker = binary.join(".cpp-infra-project.json");
        fs::write(&marker, bytes).expect("management marker");
        assert_code(
            &fixture.run(&["prepare", "--context", "linux-debug", "--output", "build/context.json"]),
            2,
        );
        assert_diagnostic(&fixture.report(), "dependency-configuration", message);
        assert_no_install(&fixture.report());
        assert_eq!(fs::read(marker).expect("unchanged marker"), bytes);
        assert!(!fixture.path().join("build/context.json").exists());
    }
}

#[test]
fn test_prepare_rejects_foreign_cmake_cache_even_with_valid_management_marker() {
    let fixture = Fixture::new();
    let outside = tempdir().expect("foreign source directory");
    let binary = fixture.path().join("build/infra/infra-debug");
    fs::create_dir_all(&binary).expect("binary directory");
    fs::write(
        binary.join(".cpp-infra-project.json"),
        json!({"schema":1,"project":fixture.path().canonicalize().expect("project")}).to_string(),
    )
    .expect("valid marker");
    let cache = format!("CMAKE_HOME_DIRECTORY:INTERNAL={}\n", outside.path().display());
    fs::write(binary.join("CMakeCache.txt"), &cache).expect("foreign CMake cache");
    assert_code(&fixture.run(&["prepare", "--context", "linux-debug"]), 2);
    assert_diagnostic(
        &fixture.report(),
        "dependency-configuration",
        "CMake cache source root differs from project",
    );
    assert_no_install(&fixture.report());
    assert_eq!(
        fs::read_to_string(binary.join("CMakeCache.txt")).expect("preserved cache"),
        cache
    );
}

#[test]
fn test_prepare_rejects_corrupt_build_state_before_install_and_preserves_bytes() {
    for (bytes, message) in [
        (b"{broken state".as_slice(), "invalid build state"),
        (b"[]".as_slice(), "build state must be an object"),
    ] {
        let fixture = Fixture::new();
        let binary = fixture.path().join("build/infra/infra-debug");
        fs::create_dir_all(&binary).expect("binary directory");
        fs::write(
            binary.join(".cpp-infra-project.json"),
            json!({"schema":1,"project":fixture.path().canonicalize().expect("project")}).to_string(),
        )
        .expect("valid marker");
        let state = binary.join(".cpp-infra-build-state.json");
        fs::write(&state, bytes).expect("corrupt build state");
        assert_code(&fixture.run(&["prepare", "--context", "linux-debug"]), 2);
        assert_diagnostic(&fixture.report(), "dependency-configuration", message);
        assert_no_install(&fixture.report());
        assert_eq!(fs::read(state).expect("preserved state"), bytes);
    }
}

#[test]
fn test_prepare_rejects_install_json_missing_layout_or_required_generators() {
    for (mutation, message) in [
        (
            " g['graph']['nodes']['0'].pop('generators_folder')",
            "Conan install JSON has no generators_folder",
        ),
        (
            " g['graph']['nodes']['0']['generators']=['CMakeToolchain']",
            "recipe must generate CMakeDeps and CMakeToolchain",
        ),
        (
            " g['graph']['nodes']['0']['generators']=[]",
            "recipe must generate CMakeDeps and CMakeToolchain",
        ),
        (
            " (dest/'conan_toolchain.cmake').unlink()",
            "recipe must generate CMakeDeps and CMakeToolchain",
        ),
    ] {
        let fixture = Fixture::new();
        adapter_replace(
            &fixture,
            " (dest/'cmakedeps_macros.cmake').write_text('# generated deps\\n')",
            &format!(" (dest/'cmakedeps_macros.cmake').write_text('# generated deps\\n')\n{mutation}"),
        );
        assert_code(
            &fixture.run(&["prepare", "--context", "linux-debug", "--output", "build/context.json"]),
            2,
        );
        let report = fixture.report();
        assert_diagnostic(&report, "dependency-configuration", message);
        assert!(
            report["commands"]
                .as_array()
                .expect("commands")
                .iter()
                .any(|command| command["args"][0] == "install")
        );
        assert!(!fixture.path().join("build/context.json").exists());
        assert!(report["data"]["dependency_context"].is_null());
    }
}

#[test]
fn test_prepare_context_output_roles_cannot_replace_management_or_generator_files() {
    for output in [
        "build/context.txt",
        "build/.cpp-infra-state.json",
        "build/CMakePresets.json",
        "build/compile_commands.json",
        "build/infra/infra-debug/conan/context.json",
        "build/report.json",
    ] {
        let fixture = Fixture::new();
        assert_code(
            &fixture.run(&["prepare", "--context", "linux-debug", "--output", output]),
            2,
        );
        assert_diagnostic(
            &fixture.report(),
            "dependency-configuration",
            "dependency context output must be a separate JSON artifact",
        );
        assert_no_install(&fixture.report());
        assert!(!fixture.path().join("build/infra/infra-debug").exists());
    }
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path().join("build/context.json")).expect("directory with JSON name");
    assert_code(
        &fixture.run(&["prepare", "--context", "linux-debug", "--output", "build/context.json"]),
        2,
    );
    assert_diagnostic(
        &fixture.report(),
        "dependency-configuration",
        "dependency context output is not a regular file",
    );
    assert_no_install(&fixture.report());
}

#[test]
fn test_sync_refuses_unsafe_text_requirements_without_replacing_recipe_or_lock() {
    for (recipe, code, diagnostic, message) in [
        (
            "[requires\nfmt/11.0.2\n",
            2,
            "dependency-configuration",
            "ambiguous Conan text section",
        ),
        (
            "[requires]\nfmt/[>=11 <12]\n",
            2,
            "dependency-configuration",
            "not a simple Conan requirement",
        ),
        (
            "[tool_requires]\nfmt/11.0.2 extra\n",
            2,
            "dependency-configuration",
            "not a simple Conan requirement",
        ),
        (
            "[requires]\nzlib/1.3.1\n",
            1,
            "dependency-gate",
            "unregistered external direct requirement zlib/1.3.1",
        ),
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.path().join("conanfile.txt"), recipe).expect("unsafe recipe");
        let old_lock = fs::read(fixture.path().join("conan/debug.lock")).expect("old lock");
        assert_code(&fixture.run(&["sync"]), code);
        let report = fixture.report();
        assert_diagnostic(&report, diagnostic, message);
        assert!(report["data"]["sync"].is_null());
        assert!(
            report["commands"]
                .as_array()
                .expect("commands")
                .iter()
                .all(|command| command["args"][0] != "graph")
        );
        assert_eq!(
            fs::read(fixture.path().join("conanfile.txt")).expect("preserved recipe"),
            recipe.as_bytes()
        );
        assert_eq!(
            fs::read(fixture.path().join("conan/debug.lock")).expect("preserved lock"),
            old_lock
        );
    }
}

#[test]
fn test_lock_update_rejects_corrupt_candidate_without_replacing_old_lock() {
    for (candidate, message) in [
        ("'{broken'", "invalid Conan lock"),
        (
            "json.dumps({'version':'0.4','requires':[],'build_requires':[],'python_requires':[]})",
            "unsupported Conan lock schema",
        ),
        (
            "json.dumps({'version':'0.5','requires':['fmt/11.0.2'],'build_requires':[],'python_requires':[]})",
            "entry must pin a recipe revision",
        ),
    ] {
        let fixture = Fixture::new();
        let old_lock = fs::read(fixture.path().join("conan/debug.lock")).expect("old lock");
        adapter_replace(
            &fixture,
            "json.dumps({'version':'0.5','requires':['fmt/11.0.2#abc%123'],'build_requires':[],'python_requires':[],'config_requires':[]})",
            candidate,
        );
        assert_code(&fixture.run(&["lock", "update", "--context", "linux-debug"]), 2);
        let report = fixture.report();
        assert_diagnostic(&report, "dependency-configuration", message);
        assert_eq!(
            fs::read(fixture.path().join("conan/debug.lock")).expect("preserved old lock"),
            old_lock
        );
        assert!(report["data"]["lock_update"].is_null());
        assert!(
            report["artifacts"]
                .as_array()
                .expect("artifacts")
                .iter()
                .all(|artifact| artifact["kind"] != "dependency-lock-candidate")
        );
    }
}

#[test]
fn test_lock_update_rejects_corrupt_historical_lock_without_replacement() {
    for (old_lock, message) in [
        ("{broken historical lock".to_string(), "invalid lock JSON"),
        (
            json!({"version":"0.5", "requires":["fmt/11.0.2#abc%123"], "build_requires":[], "python_requires":[], "config_requires":[42]}).to_string(),
            "non-string lock reference",
        ),
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.path().join("conan/debug.lock"), &old_lock).expect("historical lock");
        assert_code(
            &fixture.run(&["lock", "update", "--context", "linux-debug"]),
            2,
        );
        assert_diagnostic(&fixture.report(), "dependency-configuration", message);
        assert_eq!(
            fs::read_to_string(fixture.path().join("conan/debug.lock")).expect("preserved old lock"),
            old_lock
        );
        assert!(fixture.report()["data"]["lock_update"].is_null());
    }
}

#[test]
fn test_fam3_sync_accepts_null_parent_profile_and_preserves_dry_run_inputs() {
    let fixture = Fixture::new();
    let recipe = fixture.path().join("conanfile.txt");
    let original = fs::read_to_string(&recipe)
        .expect("recipe")
        .replace("fmt/11.0.2", "fmt/11.0.1");
    fs::write(&recipe, &original).expect("outdated recipe");
    let mut context = run_context(&fixture);
    context["profile"] = Value::Null;
    write_context(&fixture, &context);
    assert_code(
        &fixture.run(&["--run-context", "build/run-context.json", "sync", "--dry-run"]),
        0,
    );
    let report = fixture.report();
    assert_eq!(report["status"], "passed");
    for field in [
        "run_id",
        "task_id",
        "bundle_id",
        "project",
        "source_digest",
        "config_digest",
        "report_root",
    ] {
        assert_eq!(report[field], context[field], "parent identity field {field}");
    }
    assert_eq!(report["data"]["sync"]["changed"], true);
    assert_eq!(report["data"]["sync"]["dry_run"], true);
    assert_eq!(fs::read_to_string(recipe).expect("preserved recipe"), original);
    assert!(
        report["commands"]
            .as_array()
            .expect("commands")
            .iter()
            .any(|command| command["args"][0] == "graph")
    );
}

#[test]
fn test_fam3_selected_context_commands_still_require_matching_nonnull_profile() {
    let commands = [
        vec!["plan", "--context", "linux-debug"],
        vec!["inventory", "--context", "linux-debug"],
        vec!["check", "--context", "linux-debug"],
        vec!["prepare", "--context", "linux-debug"],
        vec!["lock", "update", "--context", "linux-debug"],
    ];
    for command in commands {
        for profile in [Value::Null, json!("release")] {
            let fixture = Fixture::new();
            let mut context = run_context(&fixture);
            context["profile"] = profile;
            write_context(&fixture, &context);
            let mut args = vec!["--run-context", "build/run-context.json"];
            args.extend_from_slice(&command);
            assert_code(&fixture.run(&args), 2);
            let report = fixture.report();
            assert_diagnostic(
                &report,
                "dependency-configuration",
                "run context profile differs from Conan context profile",
            );
            assert_no_install(&report);
            assert!(
                report["commands"]
                    .as_array()
                    .expect("commands")
                    .iter()
                    .all(|entry| entry["args"][0] != "graph" && entry["args"][0] != "lock")
            );
        }
    }
}

#[test]
fn test_fam3_null_sync_profile_keeps_parent_identity_digest_and_runtime_guards() {
    let fixture = Fixture::new();
    let mut valid = run_context(&fixture);
    valid["profile"] = Value::Null;
    for (field, invalid, message) in [
        ("schema", json!(2), "stale or invalid run context"),
        ("project", json!("/different-project"), "stale or invalid run context"),
        ("source_digest", json!("stale-source"), "stale or invalid run context"),
        ("config_digest", json!("stale-config"), "stale or invalid run context"),
        ("run_id", json!(""), "run context missing run_id"),
        ("task_id", json!(""), "run context missing task_id"),
        ("bundle_id", json!(""), "run context missing bundle_id"),
        (
            "report_root",
            json!(fixture.path().join("target/run")),
            "run report_root must be within project build",
        ),
        (
            "runtime_snapshot_digest",
            json!("unpaired"),
            "must be supplied together",
        ),
    ] {
        let mut context = valid.clone();
        context[field] = invalid;
        write_context(&fixture, &context);
        assert_code(
            &fixture.run(&["--run-context", "build/run-context.json", "sync", "--dry-run"]),
            2,
        );
        let report = fixture.report();
        assert_diagnostic(&report, "dependency-configuration", message);
        assert_eq!(report["commands"], json!([]));
    }
    let snapshot_path = fixture.path().join("build/fam3-runtime.json");
    for (snapshot, digest, message) in [
        (
            json!({"schema":1,"bundle_id":"lifecycle-bundle","tools":[]}),
            "wrong-hash",
            "runtime snapshot digest mismatch",
        ),
        (
            json!({"schema":1,"bundle_id":"other-bundle","tools":[]}),
            "actual",
            "runtime snapshot schema/bundle mismatch",
        ),
        (
            json!({"schema":1,"bundle_id":"lifecycle-bundle","tools":[]}),
            "actual",
            "runtime snapshot must contain one dependency tool identity",
        ),
    ] {
        let bytes = serde_json::to_vec(&snapshot).expect("snapshot bytes");
        fs::write(&snapshot_path, &bytes).expect("snapshot");
        let mut context = valid.clone();
        context["runtime_snapshot"] = json!(snapshot_path);
        context["runtime_snapshot_digest"] = if digest == "actual" {
            json!(
                Sha256::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            )
        } else {
            json!(digest)
        };
        write_context(&fixture, &context);
        assert_code(
            &fixture.run(&["--run-context", "build/run-context.json", "sync", "--dry-run"]),
            2,
        );
        let report = fixture.report();
        assert_diagnostic(&report, "dependency-configuration", message);
        assert_eq!(report["commands"], json!([]));
    }
}

#[test]
fn test_fam3_sync_rejects_missing_empty_and_wrong_type_parent_profiles() {
    let fixture = Fixture::new();
    let valid = run_context(&fixture);
    for profile in [
        None,
        Some(json!("")),
        Some(json!(42)),
        Some(json!(false)),
        Some(json!([])),
        Some(json!({})),
    ] {
        let mut context = valid.clone();
        if let Some(profile) = profile {
            context["profile"] = profile;
        } else {
            context.as_object_mut().expect("context object").remove("profile");
        }
        write_context(&fixture, &context);
        assert_code(
            &fixture.run(&["--run-context", "build/run-context.json", "sync", "--dry-run"]),
            2,
        );
        let report = fixture.report();
        assert_diagnostic(&report, "dependency-configuration", "run context missing profile");
        assert_eq!(report["commands"], json!([]));
    }
}
