// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

use std::fs;
use std::process::Command;

use serde_json::Value;
use serde_json::json;
use tempfile::tempdir;

use crate::Fixture;
use crate::assert_code;
use crate::run_with_report;

#[test]
fn test_malformed_report_and_toml_inputs_are_rejected_without_replacement() {
    let fixture = Fixture::new();
    fs::write(fixture.path().join("business.json"), "{broken JSON").expect("business file");
    assert_code(
        &run_with_report(&fixture, "business.json", &["check", "--context", "linux-debug"]),
        2,
    );
    assert_eq!(
        fs::read_to_string(fixture.path().join("business.json")).expect("business bytes"),
        "{broken JSON"
    );
    let fixture = Fixture::new();
    fs::write(fixture.path().join(".infra/project.toml"), "[broken TOML").expect("bad config");
    let output = run_with_report(&fixture, "result.json", &["check", "--context", "linux-debug"]);
    assert_code(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("project.toml"));
    assert!(!fixture.path().join("result.json").exists());
    let fixture = Fixture::new();
    fs::write(fixture.path().join("CMakePresets.json"), "{broken presets").expect("bad presets");
    let output = run_with_report(&fixture, "result.json", &["check", "--context", "linux-debug"]);
    assert_code(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid preset JSON"));
}
#[test]
fn test_report_destination_directory_and_non_directory_parent_fail_safely() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.path().join("result.json")).expect("directory output");
    assert_code(
        &run_with_report(&fixture, "result.json", &["check", "--context", "linux-debug"]),
        2,
    );
    assert!(fixture.path().join("result.json").is_dir());
    let fixture = Fixture::new();
    fs::write(fixture.path().join("reports"), "business").expect("blocking business file");
    assert_code(
        &run_with_report(&fixture, "reports/result.json", &["check", "--context", "linux-debug"]),
        3,
    );
    assert_eq!(
        fs::read_to_string(fixture.path().join("reports")).expect("preserved file"),
        "business"
    );
}
#[test]
fn test_atomic_graph_write_failure_reports_io_without_replacing_directory() {
    let fixture = Fixture::new();
    let adapter = fixture.path().join("bin/conan");
    let script=fs::read_to_string(&adapter).expect("adapter").replace("g=json.loads((p/'fixture-graph.json').read_text())","for work in (p/'build/infra-runs').glob('*/dependency/*'):\n (work/'graph.json').mkdir(exist_ok=True)\ng=json.loads((p/'fixture-graph.json').read_text())");
    fs::write(adapter, script).expect("directory-producing external recipe fixture");
    assert_code(&fixture.run(&["check", "--context", "linux-debug"]), 3);
    assert_eq!(fixture.report()["status"], "error");
    let root = std::path::PathBuf::from(fixture.report()["report_root"].as_str().expect("root"));
    assert!(
        root.join("dependency").read_dir().expect("calls").any(|entry| entry
            .expect("call")
            .path()
            .join("graph.json")
            .is_dir())
    );
}
#[test]
fn test_source_exclusion_type_must_be_a_string_array() {
    for invalid in [
        json!(null),
        json!("result.json"),
        json!([1]),
        json!({"path":"result.json"}),
    ] {
        let fixture = Fixture::new();
        assert_code(&fixture.run(&["check", "--context", "linux-debug"]), 0);
        let identity = fixture.report();
        let context = json!({"schema":1,"run_id":"bad-exclusion-type","task_id":"dependency","bundle_id":"development","project":fixture.path().canonicalize().expect("project"),"profile":"debug","source_digest":identity["source_digest"],"config_digest":identity["config_digest"],"report_root":fixture.path().join("build/infra-runs/type-check"),"source_excluded_paths":invalid});
        fs::write(fixture.path().join("build/context.json"), context.to_string()).expect("context");
        let output = fixture.run(&[
            "--run-context",
            "build/context.json",
            "check",
            "--context",
            "linux-debug",
        ]);
        assert_code(&output, 2);
        assert!(String::from_utf8_lossy(&output.stderr).contains("array of file paths"));
    }
}
#[test]
fn test_external_context_output_is_rejected_and_existing_business_bytes_remain() {
    let fixture = Fixture::new();
    let outside = tempdir().expect("outside");
    let output = outside.path().join("context.json");
    fs::write(&output, "{\"business\":true}").expect("external business");
    let result = fixture.run(&[
        "prepare",
        "--context",
        "linux-debug",
        "--output",
        output.to_str().expect("UTF8"),
    ]);
    assert_code(&result, 2);
    assert!(String::from_utf8_lossy(&result.stderr).contains("outside project"));
    assert_eq!(
        fs::read_to_string(output).expect("external file"),
        "{\"business\":true}"
    );
}
#[cfg(unix)]
#[test]
fn test_non_utf8_source_filename_is_a_configuration_failure() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    fs::write(
        fixture.path().join("src").join(OsString::from_vec(vec![0xff])),
        "source",
    )
    .expect("nonUTF8 filename");
    let output = fixture.run(&["check", "--context", "linux-debug"]);
    assert_code(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("non-UTF8 project path"));
}
#[cfg(unix)]
#[test]
fn test_non_utf8_output_paths_are_rejected_without_conan_install() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    for report in [false, true] {
        let fixture = Fixture::new();
        let mut command = Command::new(env!("CARGO_BIN_EXE_cpp-infra-dependency"));
        command.arg("--project").arg(fixture.path());
        if report {
            command
                .arg("--report")
                .arg(fixture.path().join(OsString::from_vec(b"result-\xff.json".to_vec())))
                .args(["check", "--context", "linux-debug"]);
        } else {
            command
                .args([
                    "--report",
                    "build/report.json",
                    "prepare",
                    "--context",
                    "linux-debug",
                    "--output",
                ])
                .arg(
                    fixture
                        .path()
                        .join("build")
                        .join(OsString::from_vec(b"context-\xff.json".to_vec())),
                );
        }
        let output = command
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    fixture.path().join("bin").display(),
                    std::env::var("PATH").expect("PATH")
                ),
            )
            .output()
            .expect("CLI");
        assert_code(&output, 2);
        assert!(String::from_utf8_lossy(&output.stderr).contains("UTF-8"));
        assert!(!fixture.path().join("build/infra/infra-debug").exists());
    }
}
#[cfg(unix)]
#[test]
fn test_non_utf8_generated_filename_is_rejected_as_unusable_build_input() {
    let fixture = Fixture::new();
    let adapter = fixture.path().join("bin/conan");
    let script=fs::read_to_string(&adapter).expect("adapter").replace("(dest/'cmakedeps_macros.cmake').write_text('# generated deps\\n')","(dest/'cmakedeps_macros.cmake').write_text('# generated deps\\n')\n fd=os.open(os.fsencode(dest)+b'/\\xff.cmake',os.O_CREAT|os.O_WRONLY,0o600)\n os.close(fd)");
    fs::write(adapter, script).expect("generator fixture");
    let output = fixture.run(&["prepare", "--context", "linux-debug"]);
    assert_code(&output, 2);
    assert!(String::from_utf8_lossy(&output.stderr).contains("UTF-8"));
}
#[test]
fn test_report_validation_rejects_non_public_json_shapes() {
    for value in [
        json!(null),
        json!([]),
        json!({"schema":1,"tool":{"name":"business","version":"1","revision":null}}),
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.path().join("business.json"), value.to_string()).expect("business JSON");
        assert_code(
            &run_with_report(&fixture, "business.json", &["check", "--context", "linux-debug"]),
            2,
        );
        let original: Value =
            serde_json::from_slice(&fs::read(fixture.path().join("business.json")).expect("business")).expect("JSON");
        assert_eq!(original, value);
    }
}
