// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

use std::fs;
use std::path::Path;
use std::process::Command;
use std::process::Output;

use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use tempfile::tempdir;

mod cli;

struct Fixture {
    root: TempDir,
}
impl Fixture {
    fn new() -> Self {
        let root = tempdir().expect("fixture directory");
        let p = root.path();
        for directory in [
            ".infra/dependency/policy",
            ".infra/dependency/profiles",
            "conan",
            "bin",
            "src",
        ] {
            fs::create_dir_all(p.join(directory)).expect("fixture directory");
        }
        fs::write(p.join(".infra/project.toml"), "schema = 1\nkind = 'library'\nsource_roots = ['src']\ntest_roots = []\ngenerated_roots = []\nthird_party_roots = []\n[profiles.debug]\nconfigure = 'infra-debug'\nbuild = 'infra-debug'\ntest = 'infra-debug'\nconan_context = 'linux-debug'\n").expect("project config");
        fs::write(p.join(".infra/dependency/config.toml"), "schema = 1\nbackend = 'conan2'\nrecipe = 'conanfile.txt'\npolicy = '.infra/dependency/policy/baseline.toml'\n[contexts.linux-debug]\nbuild_profile = '.infra/dependency/profiles/build'\nhost_profile = '.infra/dependency/profiles/host'\nlockfile = 'conan/debug.lock'\nprofile = 'debug'\ngenerators_dir = 'build/infra/infra-debug/conan'\nbuild_missing = false\nremotes = []\n").expect("dependency config");
        fs::write(p.join(".infra/dependency/policy/baseline.toml"), "schema = 1\nbaseline_id = 'test-1'\ninternal_patterns = ['qubit-*']\nallow_refs = []\n[direct]\nfmt = 'fmt/11.0.2'\n[resolved]\nfmt = '>=11.0.0,<12.0.0'\n").expect("policy");
        for profile in ["build", "host"] {
            fs::write(p.join(format!(".infra/dependency/profiles/{profile}")), "[settings]\nos=Linux\narch=x86_64\ncompiler=gcc\ncompiler.version=13\ncompiler.libcxx=libstdc++11\nbuild_type=Debug\ncompiler.cppstd=20\n").expect("profile");
        }
        fs::write(p.join("CMakePresets.json"), json!({"version":6,"configurePresets":[{"name":"infra-debug","generator":"Ninja","binaryDir":"${sourceDir}/build/infra/infra-debug","toolchainFile":"${sourceDir}/build/infra/infra-debug/conan/conan_toolchain.cmake"}]}).to_string()).expect("presets");
        fs::write(
            p.join("conanfile.txt"),
            "# recipe\n[requires]\nfmt/11.0.2 # keep me\n[generators]\nCMakeDeps\nCMakeToolchain\n",
        )
        .expect("recipe");
        fs::write(p.join("conan/debug.lock"), json!({"version":"0.5","requires":["fmt/11.0.2#abc%123"],"build_requires":[],"python_requires":[],"config_requires":[]}).to_string()).expect("lock");
        let graph = json!({"graph":{"nodes":{"0":{"ref":null,"context":"host","package_id":null,"generators":["CMakeDeps","CMakeToolchain"],"generators_folder":p.join("build/infra/infra-debug/conan"),"settings":{"compiler":"gcc","compiler.version":"13","build_type":"Debug","compiler.cppstd":"20"},"dependencies":{"1":{"ref":"fmt/11.0.2#abc","require":"fmt/11.0.2","direct":true,"build":false,"test":false}}},"1":{"ref":"fmt/11.0.2#abc","context":"host","package_id":"package1","binary":"Cache","dependencies":{}}}}});
        fs::write(p.join("fixture-graph.json"), graph.to_string()).expect("graph");
        fs::write(p.join("bin/conan"), r##"#!/usr/bin/env python3
import json, os, pathlib, sys
p=pathlib.Path(os.getcwd())
a=sys.argv[1:]
if a == ['--version']:
 print('Conan version 2.28.0'); sys.exit(0)
if a[:2] == ['remote','list']:
 print('[]'); sys.exit(0)
if (p/'fail-conan').exists():
 print('fixture Conan failure',file=sys.stderr); sys.exit(1)
g=json.loads((p/'fixture-graph.json').read_text())
if a[:2] == ['lock','create']:
 dest=a[a.index('--lockfile-out')+1]
 pathlib.Path(dest).write_text(json.dumps({'version':'0.5','requires':['fmt/11.0.2#abc%123'],'build_requires':[],'python_requires':[],'config_requires':[]}))
 sys.exit(0)
if a and a[0]=='install':
 dest=pathlib.Path(g['graph']['nodes']['0']['generators_folder']); dest.mkdir(parents=True,exist_ok=True)
 (dest/'conan_toolchain.cmake').write_text('# generated toolchain\n')
 (dest/'cmakedeps_macros.cmake').write_text('# generated deps\n')
print(json.dumps(g))
"##).expect("fake conan");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(p.join("bin/conan"), fs::Permissions::from_mode(0o755)).expect("executable");
        }
        Self { root }
    }
    fn path(&self) -> &Path {
        self.root.path()
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cpp-infra-dependency"))
            .arg("--project")
            .arg(self.path())
            .arg("--report")
            .arg(self.path().join("build/report.json"))
            .args(args)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.path().join("bin").display(),
                    std::env::var("PATH").expect("PATH")
                ),
            )
            .output()
            .expect("run CLI")
    }
    fn report(&self) -> Value {
        serde_json::from_slice(&fs::read(self.path().join("build/report.json")).expect("report exists"))
            .expect("valid report")
    }
    fn graph(&self) -> Value {
        serde_json::from_slice(&fs::read(self.path().join("fixture-graph.json")).expect("graph")).expect("graph json")
    }
    fn put_graph(&self, graph: &Value) {
        fs::write(self.path().join("fixture-graph.json"), graph.to_string()).expect("update graph");
    }
}
fn assert_code(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_inventory_reports_graph_and_complete_common_identity() {
    let f = Fixture::new();
    let o = f.run(&["inventory", "--context", "linux-debug"]);
    assert_code(&o, 0);
    let r = f.report();
    assert_eq!(r["status"], "passed");
    assert_eq!(r["schema"], 1);
    assert_eq!(r["bundle_id"], "development");
    for k in [
        "source_digest",
        "config_digest",
        "report_root",
        "started_at",
        "commands",
        "external_versions",
    ] {
        assert!(!r[k].is_null(), "missing {k}");
    }
    assert_eq!(r["data"]["inventory"]["nodes"][0]["recipe_revision"], "abc");
    assert!(
        r["artifacts"]
            .as_array()
            .expect("artifacts")
            .iter()
            .any(|a| a["kind"] == "dependency-graph")
    );
}
#[test]
fn test_prepare_writes_verified_context_and_toolchain() {
    let f = Fixture::new();
    let o = f.run(&[
        "prepare",
        "--context",
        "linux-debug",
        "--output",
        "build/dependency-context.json",
    ]);
    assert_code(&o, 0);
    let c: Value = serde_json::from_slice(&fs::read(f.path().join("build/dependency-context.json")).expect("context"))
        .expect("json");
    for k in [
        "source_digest",
        "config_digest",
        "baseline_id",
        "lock_digest",
        "graph_digest",
        "fingerprint",
        "generated_files",
    ] {
        assert!(!c[k].is_null(), "missing {k}");
    }
    assert_eq!(c["profile"], "debug");
    assert_eq!(c["context"], "linux-debug");
    assert!(
        f.report()["commands"]
            .as_array()
            .expect("commands")
            .iter()
            .any(|c| c["args"].as_array().expect("args").contains(&json!("--build=never")))
    );
}
#[test]
fn test_resolved_policy_catches_build_context_same_name() {
    let f = Fixture::new();
    let mut g = f.graph();
    g["graph"]["nodes"]["2"] =
        json!({"ref":"fmt/10.0.0#old","context":"build","package_id":"tool","binary":"Cache","dependencies":{}});
    f.put_graph(&g);
    let o = f.run(&["check", "--context", "linux-debug"]);
    assert_code(&o, 1);
    assert!(f.report()["diagnostics"].to_string().contains("resolved"));
}
#[test]
fn test_tool_requires_cannot_bypass_direct_policy() {
    let f = Fixture::new();
    let mut g = f.graph();
    g["graph"]["nodes"]["0"]["dependencies"]["2"] =
        json!({"ref":"cmake/3.28.0#r","require":"cmake/3.28.0","direct":true,"build":true,"test":false});
    g["graph"]["nodes"]["2"] =
        json!({"ref":"cmake/3.28.0#r","context":"build","package_id":"tool","binary":"Cache","dependencies":{}});
    f.put_graph(&g);
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 1);
    assert!(f.report()["diagnostics"].to_string().contains("unregistered"));
}
#[test]
fn test_missing_lock_rejected_before_conan() {
    let f = Fixture::new();
    fs::rename(f.path().join("conan/debug.lock"), f.path().join("conan/old.lock")).expect("hide lock");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
    assert!(f.report()["diagnostics"].to_string().contains("lock update"));
}
#[test]
fn test_lock_missing_reference_is_failure() {
    let f = Fixture::new();
    fs::write(
        f.path().join("conan/debug.lock"),
        r#"{"version":"0.5","requires":[],"build_requires":[],"python_requires":[]}"#,
    )
    .expect("empty lock");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 1);
}
#[test]
fn test_prepare_rejects_recipe_layout_drift() {
    let f = Fixture::new();
    let mut g = f.graph();
    g["graph"]["nodes"]["0"]["generators_folder"] = json!(f.path().join("build/infra/infra-debug/wrong"));
    f.put_graph(&g);
    assert_code(
        &f.run(&["prepare", "--context", "linux-debug", "--output", "build/context.json"]),
        2,
    );
    assert!(!f.path().join("build/context.json").exists());
}
#[test]
fn test_config_typo_and_symlink_escape_rejected() {
    let f = Fixture::new();
    let p = f.path().join(".infra/dependency/config.toml");
    let mut s = fs::read_to_string(&p).expect("config");
    s.push_str("typo = true\n");
    fs::write(p, s).expect("typo");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
    #[cfg(unix)]
    {
        let f = Fixture::new();
        let outside = tempdir().expect("outside");
        fs::write(outside.path().join("host"), "[settings]\n").expect("external");
        fs::rename(
            f.path().join(".infra/dependency/profiles/host"),
            f.path().join(".infra/dependency/profiles/saved"),
        )
        .expect("save");
        std::os::unix::fs::symlink(
            outside.path().join("host"),
            f.path().join(".infra/dependency/profiles/host"),
        )
        .expect("link");
        assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
    }
}
#[test]
fn test_sync_preserves_comments_and_dry_run() {
    let f = Fixture::new();
    let p = f.path().join("conanfile.txt");
    let s = fs::read_to_string(&p)
        .expect("recipe")
        .replace("fmt/11.0.2", "fmt/10.0.0");
    fs::write(&p, &s).expect("old version");
    assert_code(&f.run(&["sync", "--dry-run"]), 0);
    assert_eq!(fs::read_to_string(&p).expect("unchanged"), s);
    assert_code(&f.run(&["sync"]), 0);
    let s = fs::read_to_string(&p).expect("updated");
    assert!(s.contains("fmt/11.0.2 # keep me"));
    assert!(s.contains("[generators]\nCMakeDeps\nCMakeToolchain"));
}
#[test]
fn test_sync_python_refuses_automatic_edit() {
    let f = Fixture::new();
    let p = f.path().join(".infra/dependency/config.toml");
    fs::write(
        &p,
        fs::read_to_string(&p)
            .expect("config")
            .replace("conanfile.txt", "conanfile.py"),
    )
    .expect("python config");
    fs::write(
        f.path().join("conanfile.py"),
        "from conan import ConanFile\nclass P(ConanFile): pass\n",
    )
    .expect("python recipe");
    assert_code(&f.run(&["sync"]), 2);
}
#[test]
fn test_lock_update_dry_run_and_failure_preserve_old_lock() {
    let f = Fixture::new();
    let p = f.path().join("conan/debug.lock");
    let old = fs::read(&p).expect("old");
    assert_code(&f.run(&["lock", "update", "--context", "linux-debug", "--dry-run"]), 0);
    assert_eq!(fs::read(&p).expect("unchanged"), old);
    fs::write(f.path().join("fail-conan"), "").expect("failure trigger");
    assert_code(&f.run(&["lock", "update", "--context", "linux-debug"]), 1);
    assert_eq!(fs::read(p).expect("old kept"), old);
}
#[test]
fn test_offline_passes_no_remote_and_conan_failure_is_reported() {
    let f = Fixture::new();
    assert_code(&f.run(&["--offline", "check", "--context", "linux-debug"]), 0);
    assert!(f.report()["commands"].to_string().contains("--no-remote"));
    fs::write(f.path().join("fail-conan"), "").expect("failure trigger");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 1);
    assert_eq!(f.report()["status"], "failed");
}
#[test]
fn test_version_json_has_null_development_revision() {
    let o = Command::new(env!("CARGO_BIN_EXE_cpp-infra-dependency"))
        .args(["--version", "--json"])
        .output()
        .expect("version");
    assert_code(&o, 0);
    let v: Value = serde_json::from_slice(&o.stdout).expect("version json");
    assert_eq!(v["protocol"], 1);
    assert_eq!(v["name"], "cpp-infra-dependency");
    assert!(v["revision"].is_null());
}

#[test]
fn test_stale_run_context_source_is_rejected() {
    let f = Fixture::new();
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
    let r = f.report();
    let c = json!({"schema":1,"run_id":"shared-run","task_id":"dependency-task","bundle_id":"development","project":f.path().canonicalize().expect("project"),"profile":"debug","source_digest":r["source_digest"],"config_digest":r["config_digest"],"report_root":f.path().join("build/infra-runs/shared-run")});
    fs::write(f.path().join("build/run-context.json"), c.to_string()).expect("run context");
    assert_code(
        &f.run(&[
            "--run-context",
            "build/run-context.json",
            "check",
            "--context",
            "linux-debug",
        ]),
        0,
    );
    fs::write(f.path().join("src/changed.cpp"), "int changed;\n").expect("source change");
    assert_code(
        &f.run(&[
            "--run-context",
            "build/run-context.json",
            "check",
            "--context",
            "linux-debug",
        ]),
        2,
    );
    assert!(f.report()["diagnostics"].to_string().contains("stale"));
}
#[test]
fn test_check_python_recipe_preserves_project_inputs() {
    let f = Fixture::new();
    let p = f.path().join(".infra/dependency/config.toml");
    fs::write(
        &p,
        fs::read_to_string(&p)
            .expect("config")
            .replace("conanfile.txt", "conanfile.py"),
    )
    .expect("python config");
    fs::write(
        f.path().join("conanfile.py"),
        "from conan import ConanFile\nclass P(ConanFile): pass\n",
    )
    .expect("python");
    let paths = [
        "conanfile.py",
        "conan/debug.lock",
        "CMakePresets.json",
        ".infra/dependency/config.toml",
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|p| fs::read(f.path().join(p)).expect("input"))
        .collect();
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
    for (p, bytes) in paths.iter().zip(before) {
        assert_eq!(fs::read(f.path().join(p)).expect("unchanged"), bytes);
    }
}
#[test]
fn test_internal_direct_is_exempt_but_resolved_still_enforced() {
    let f = Fixture::new();
    let mut g = f.graph();
    g["graph"]["nodes"]["1"]["ref"] = json!("qubit-fmt/11.0.2#abc");
    g["graph"]["nodes"]["0"]["dependencies"]["1"]["ref"] = json!("qubit-fmt/11.0.2#abc");
    g["graph"]["nodes"]["0"]["dependencies"]["1"]["require"] = json!("qubit-fmt/11.0.2");
    f.put_graph(&g);
    fs::write(
        f.path().join("conan/debug.lock"),
        r#"{"version":"0.5","requires":["qubit-fmt/11.0.2#abc"],"build_requires":[],"python_requires":[]}"#,
    )
    .expect("lock");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
    let p = f.path().join(".infra/dependency/policy/baseline.toml");
    let s = fs::read_to_string(&p).expect("policy") + "qubit-fmt = '<11.0.0'\n";
    fs::write(p, s).expect("strict internal resolved");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 1);
}
#[test]
fn test_non_numeric_versions_require_exact_allow_ref() {
    let f = Fixture::new();
    let mut g = f.graph();
    g["graph"]["nodes"]["1"]["ref"] = json!("fmt/dev.5@user/stable#abc");
    g["graph"]["nodes"]["0"]["dependencies"]["1"]["ref"] = json!("fmt/dev.5@user/stable#abc");
    g["graph"]["nodes"]["0"]["dependencies"]["1"]["require"] = json!("fmt/dev.5@user/stable");
    f.put_graph(&g);
    let p = f.path().join(".infra/dependency/policy/baseline.toml");
    fs::write(
        &p,
        fs::read_to_string(&p)
            .expect("policy")
            .replace("fmt/11.0.2", "fmt/dev.5@user/stable"),
    )
    .expect("direct policy");
    fs::write(
        f.path().join("conan/debug.lock"),
        r#"{"version":"0.5","requires":["fmt/dev.5@user/stable#abc"],"build_requires":[],"python_requires":[]}"#,
    )
    .expect("lock");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 1);
    fs::write(
        &p,
        fs::read_to_string(&p)
            .expect("policy")
            .replace("allow_refs = []", "allow_refs = ['fmt/dev.5@user/stable#abc']"),
    )
    .expect("allow exact");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
}
#[test]
fn test_sync_validation_failure_keeps_original_recipe() {
    let f = Fixture::new();
    let p = f.path().join("conanfile.txt");
    let old = fs::read_to_string(&p)
        .expect("recipe")
        .replace("fmt/11.0.2", "fmt/10.0.0");
    fs::write(&p, &old).expect("old recipe");
    fs::write(f.path().join("fail-conan"), "").expect("failure");
    assert_code(&f.run(&["sync"]), 1);
    assert_eq!(fs::read_to_string(p).expect("kept recipe"), old);
}
#[test]
fn test_lock_update_policy_failure_keeps_old_lock() {
    let f = Fixture::new();
    let p = f.path().join("conan/debug.lock");
    let old = fs::read(&p).expect("old lock");
    let mut g = f.graph();
    g["graph"]["nodes"]["1"]["ref"] = json!("fmt/10.0.0#old");
    f.put_graph(&g);
    assert_code(&f.run(&["lock", "update", "--context", "linux-debug"]), 1);
    assert_eq!(fs::read(p).expect("old lock kept"), old);
}
#[test]
fn test_unmarked_binary_dir_is_not_taken_over() {
    let f = Fixture::new();
    fs::create_dir_all(f.path().join("build/infra/infra-debug")).expect("binary dir");
    fs::write(f.path().join("build/infra/infra-debug/user-data"), "owned by user").expect("user data");
    assert_code(&f.run(&["prepare", "--context", "linux-debug"]), 2);
    assert!(
        !f.path()
            .join("build/infra/infra-debug/.cpp-infra-project.json")
            .exists()
    );
}
#[test]
fn test_malformed_graph_is_protocol_error() {
    let f = Fixture::new();
    let mut g = f.graph();
    g["graph"]["nodes"]["1"]
        .as_object_mut()
        .expect("node")
        .remove("package_id");
    f.put_graph(&g);
    assert_code(&f.run(&["inventory", "--context", "linux-debug"]), 2);
}
#[test]
fn test_plan_does_not_spawn_any_external_program() {
    let f = Fixture::new();
    let git = f.path().join("bin/git");
    fs::write(
        &git,
        "#!/usr/bin/env python3\nimport pathlib\npathlib.Path('external-was-run').write_text('yes')\n",
    )
    .expect("git probe");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(git, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    assert_code(&f.run(&["plan", "--context", "linux-debug"]), 0);
    assert!(!f.path().join("external-was-run").exists(), "plan spawned git");
    assert!(f.report()["commands"].as_array().expect("commands").is_empty());
}
#[test]
fn test_runtime_snapshot_digest_and_executable_identity_are_verified() {
    use sha2::Digest;
    use sha2::Sha256;
    let f = Fixture::new();
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
    let r = f.report();
    let binary = std::path::PathBuf::from(env!("CARGO_BIN_EXE_cpp-infra-dependency"))
        .canonicalize()
        .expect("binary");
    let s = json!({"schema":1,"bundle_id":"main-test","tools":[{"name":"cpp-infra-dependency","version":"0.1.0","revision":null,"protocol":1,"binary":binary,"executable_sha256":Sha256::digest(fs::read(&binary).expect("binary bytes")).iter().map(|byte| format!("{byte:02x}")).collect::<String>()}],"offline":true,"stale":false});
    let path = f.path().join("build/runtime.json");
    fs::write(&path, s.to_string()).expect("snapshot");
    let c = json!({"schema":1,"run_id":"runtime-run","task_id":"dependency","bundle_id":"main-test","project":f.path().canonicalize().expect("project"),"profile":"debug","source_digest":r["source_digest"],"config_digest":r["config_digest"],"report_root":f.path().join("build/infra-runs/runtime-run"),"runtime_snapshot":path,"runtime_snapshot_digest":Sha256::digest(fs::read(&path).expect("snapshot bytes")).iter().map(|byte| format!("{byte:02x}")).collect::<String>()});
    fs::write(f.path().join("build/runtime-context.json"), c.to_string()).expect("context");
    assert_code(
        &f.run(&[
            "--run-context",
            "build/runtime-context.json",
            "check",
            "--context",
            "linux-debug",
        ]),
        0,
    );
    fs::write(path, "{}").expect("tampered snapshot");
    assert_code(
        &f.run(&[
            "--run-context",
            "build/runtime-context.json",
            "check",
            "--context",
            "linux-debug",
        ]),
        2,
    );
}
#[test]
fn test_prepare_requires_selected_preset_toolchain_binding() {
    let f = Fixture::new();
    let p = f.path().join("CMakePresets.json");
    let mut presets: Value = serde_json::from_slice(&fs::read(&p).expect("presets")).expect("json");
    presets["configurePresets"][0]["toolchainFile"] = json!("${sourceDir}/unrelated-toolchain.cmake");
    fs::write(p, presets.to_string()).expect("wrong toolchain");
    assert_code(&f.run(&["prepare", "--context", "linux-debug"]), 2);
    assert!(f.report()["diagnostics"].to_string().contains("toolchain"));
}
#[test]
fn test_check_rejects_inputs_changed_during_recipe_execution() {
    let f = Fixture::new();
    let p = f.path().join("bin/conan");
    let script = fs::read_to_string(&p).expect("fake conan").replace(
        "g=json.loads((p/'fixture-graph.json').read_text())",
        "g=json.loads((p/'fixture-graph.json').read_text())\n(p/'src/mutated.cpp').write_text('int mutated;')",
    );
    fs::write(p, script).expect("mutating recipe adapter");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
    assert!(f.report()["diagnostics"].to_string().contains("changed"));
}
#[test]
fn test_timeout_kills_conan_and_preserves_error_report() {
    let f = Fixture::new();
    let p = f.path().join(".infra/dependency/config.toml");
    fs::write(
        &p,
        fs::read_to_string(&p)
            .expect("config")
            .replace("backend = 'conan2'", "backend = 'conan2'\ntimeout_seconds = 1"),
    )
    .expect("timeout config");
    let p = f.path().join("bin/conan");
    fs::write(
        &p,
        fs::read_to_string(&p).expect("fake conan").replace(
            "g=json.loads((p/'fixture-graph.json').read_text())",
            "import time\ntime.sleep(10)\ng=json.loads((p/'fixture-graph.json').read_text())",
        ),
    )
    .expect("slow conan");
    let start = std::time::Instant::now();
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 1);
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert!(f.report()["diagnostics"].to_string().contains("timed out"));
}
#[test]
fn test_nested_build_source_is_hashed_and_root_build_is_ignored() {
    let f = Fixture::new();
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
    let before = f.report()["source_digest"].clone();
    fs::create_dir_all(f.path().join("src/build")).expect("source subtree");
    fs::write(f.path().join("src/build/real.cpp"), "int nested;").expect("source");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
    assert_ne!(f.report()["source_digest"], before, "nested build source was omitted");
    let before = f.report()["source_digest"].clone();
    fs::write(f.path().join("build/disposable.log"), "cache").expect("cache");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
    assert_eq!(f.report()["source_digest"], before);
}
#[test]
fn test_policy_accepts_explicit_loopback_remote_and_enforces_actual_url() {
    let f = Fixture::new();
    let p = f.path().join(".infra/dependency/config.toml");
    fs::write(
        &p,
        fs::read_to_string(&p)
            .expect("config")
            .replace("remotes = []", "remotes = ['fixture']"),
    )
    .expect("remote config");
    let p = f.path().join(".infra/dependency/policy/baseline.toml");
    fs::write(
        &p,
        fs::read_to_string(&p).expect("policy") + "\n[remotes]\nfixture='http://127.0.0.1:9300'\n",
    )
    .expect("remote policy");
    let p = f.path().join("bin/conan");
    fs::write(
        &p,
        fs::read_to_string(&p).expect("fake conan").replace(
            "print('[]'); sys.exit(0)",
            "print('[{\"name\":\"fixture\",\"url\":\"http://127.0.0.1:9300\",\"enabled\":true}]'); sys.exit(0)",
        ),
    )
    .expect("remote listing");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
    fs::write(
        &p,
        fs::read_to_string(&p)
            .expect("fake conan")
            .replace("127.0.0.1:9300", "127.0.0.1:9400"),
    )
    .expect("changed actual remote");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
}
#[cfg(unix)]
#[test]
fn test_relative_report_cannot_follow_project_build_symlink_outside() {
    let f = Fixture::new();
    let outside = tempdir().expect("outside");
    std::os::unix::fs::symlink(outside.path(), f.path().join("build")).expect("build symlink");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
    assert!(!outside.path().join("report.json").exists(), "report escaped project");
}
#[test]
fn test_parallel_prepare_serializes_same_binary_dir() {
    let f = Fixture::new();
    let p = f.path().join("bin/conan");
    let script=fs::read_to_string(&p).expect("fake conan").replace("if a and a[0]=='install':","if a and a[0]=='install':\n import time\n active=p/'build/install-active'\n if active.exists(): sys.exit(7)\n active.write_text('active')\n time.sleep(0.2)\n active.unlink()");
    fs::write(p, script).expect("race detector");
    std::thread::scope(|scope| {
        let first = scope.spawn(|| f.run(&["prepare", "--context", "linux-debug", "--output", "build/context1.json"]));
        let second = scope.spawn(|| f.run(&["prepare", "--context", "linux-debug", "--output", "build/context2.json"]));
        assert_code(&first.join().expect("thread1"), 0);
        assert_code(&second.join().expect("thread2"), 0);
    });
}
#[test]
fn test_binary_lock_wait_obeys_timeout() {
    use fs2::FileExt;
    let f = Fixture::new();
    let p = f.path().join(".infra/dependency/config.toml");
    fs::write(
        &p,
        fs::read_to_string(&p)
            .expect("config")
            .replace("backend = 'conan2'", "backend = 'conan2'\ntimeout_seconds = 1"),
    )
    .expect("timeout config");
    fs::create_dir_all(f.path().join("build/infra/infra-debug")).expect("binary dir");
    let guard = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(f.path().join("build/infra/infra-debug/.cpp-infra.lock"))
        .expect("lock file");
    guard.lock_exclusive().expect("lock");
    assert_code(&f.run(&["prepare", "--context", "linux-debug"]), 1);
    assert!(
        f.report()["diagnostics"]
            .to_string()
            .contains("waiting for project lock")
    );
}
#[cfg(unix)]
#[test]
fn test_cancelled_conan_writes_cancelled_report() {
    let f = Fixture::new();
    let p = f.path().join("bin/conan");
    fs::write(&p,fs::read_to_string(&p).expect("fake conan").replace("g=json.loads((p/'fixture-graph.json').read_text())","import time\n(p/'build/recipe-running').write_text('running')\ntime.sleep(20)\ng=json.loads((p/'fixture-graph.json').read_text())")).expect("slow recipe");
    let mut child = Command::new(env!("CARGO_BIN_EXE_cpp-infra-dependency"))
        .arg("--project")
        .arg(f.path())
        .args(["--report", "build/report.json", "check", "--context", "linux-debug"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                f.path().join("bin").display(),
                std::env::var("PATH").expect("PATH")
            ),
        )
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("CLI");
    let start = std::time::Instant::now();
    while !f.path().join("build/recipe-running").exists() {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "recipe never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    assert_eq!(child.wait().expect("cancelled CLI").code(), Some(130));
    assert_eq!(f.report()["status"], "cancelled");
}
#[test]
fn test_prepare_invalidates_previous_build_state_before_reinstall() {
    let f = Fixture::new();
    assert_code(&f.run(&["prepare", "--context", "linux-debug"]), 0);
    let state = f.path().join("build/infra/infra-debug/.cpp-infra-build-state.json");
    fs::write(
        &state,
        json!({"schema":1,"project":f.path(),"fingerprint":"old-built-context","status":"passed"}).to_string(),
    )
    .expect("build state");
    assert_code(&f.run(&["prepare", "--context", "linux-debug"]), 0);
    let value: Value = serde_json::from_slice(&fs::read(state).expect("state")).expect("json");
    assert!(value["fingerprint"].is_null());
    assert_eq!(value["status"], "dependency-updating");
}
#[cfg(unix)]
#[test]
fn test_prepare_rejects_generated_file_symlink_outside_generators() {
    let f = Fixture::new();
    let p = f.path().join("bin/conan");
    let script=fs::read_to_string(&p).expect("fake conan").replace("(dest/'conan_toolchain.cmake').write_text('# generated toolchain\\n')","(p/'build/outside-toolchain.cmake').write_text('# elsewhere')\n (dest/'conan_toolchain.cmake').symlink_to(p/'build/outside-toolchain.cmake')");
    fs::write(p, script).expect("symlink generator");
    assert_code(&f.run(&["prepare", "--context", "linux-debug"]), 2);
    assert!(f.report()["diagnostics"].to_string().contains("symlink"));
}
#[test]
fn test_shared_run_context_keeps_each_invocations_artifacts_separate() {
    let f = Fixture::new();
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
    let r = f.report();
    let root = f.path().join("build/infra-runs/shared-artifacts");
    let c = json!({"schema":1,"run_id":"shared-artifacts","task_id":"dependency","bundle_id":"development","project":f.path().canonicalize().expect("project"),"profile":"debug","source_digest":r["source_digest"],"config_digest":r["config_digest"],"report_root":root});
    fs::write(f.path().join("build/shared-context.json"), c.to_string()).expect("context");
    assert_code(
        &f.run(&[
            "--run-context",
            "build/shared-context.json",
            "check",
            "--context",
            "linux-debug",
        ]),
        0,
    );
    let first = f.report();
    assert_code(
        &f.run(&[
            "--run-context",
            "build/shared-context.json",
            "check",
            "--context",
            "linux-debug",
        ]),
        0,
    );
    let second = f.report();
    let first_paths: Vec<_> = first["artifacts"]
        .as_array()
        .expect("artifacts")
        .iter()
        .map(|a| a["path"].as_str().expect("path"))
        .collect();
    for a in second["artifacts"].as_array().expect("artifacts") {
        assert!(
            !first_paths.contains(&a["path"].as_str().expect("path")),
            "shared artifact was reused: {}",
            a["path"]
        );
    }
    for a in first["artifacts"].as_array().expect("artifacts") {
        use sha2::Digest;
        use sha2::Sha256;
        let path = root.join(a["path"].as_str().expect("path"));
        assert_eq!(
            a["sha256"],
            Sha256::digest(fs::read(path).expect("original artifact"))
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
    }
}
#[test]
fn test_profile_bare_include_is_rejected_to_prevent_conan_home_shadowing() {
    let f = Fixture::new();
    fs::write(
        f.path().join(".infra/dependency/profiles/common"),
        "[settings]\nbuild_type=Debug\n",
    )
    .expect("local include");
    fs::write(f.path().join(".infra/dependency/profiles/host"), "include(common)\n").expect("bare include");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
    assert!(f.report()["diagnostics"].to_string().contains("include"));
}
#[test]
fn test_consumed_profile_in_ignored_build_tree_is_rejected() {
    let f = Fixture::new();
    fs::create_dir_all(f.path().join("build")).expect("build");
    fs::copy(
        f.path().join(".infra/dependency/profiles/host"),
        f.path().join("build/host"),
    )
    .expect("ignored profile");
    let p = f.path().join(".infra/dependency/config.toml");
    fs::write(
        &p,
        fs::read_to_string(&p).expect("config").replace(
            "host_profile = '.infra/dependency/profiles/host'",
            "host_profile = 'build/host'",
        ),
    )
    .expect("config");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
    assert!(f.report()["diagnostics"].to_string().contains("digest"));
}
#[test]
fn test_preset_inherited_cache_variables_preserve_toolchain_binding() {
    let f = Fixture::new();
    let p = f.path().join("CMakePresets.json");
    let presets = json!({"version":6,"configurePresets":[{"name":"base","hidden":true,"generator":"Ninja","binaryDir":"${sourceDir}/build/infra/infra-debug","cacheVariables":{"CMAKE_TOOLCHAIN_FILE":"${sourceDir}/build/infra/infra-debug/conan/conan_toolchain.cmake","BASE_ONLY":"kept"}},{"name":"infra-debug","inherits":"base","cacheVariables":{"CMAKE_BUILD_TYPE":"Debug"}}]});
    fs::write(p, presets.to_string()).expect("presets");
    assert_code(&f.run(&["prepare", "--context", "linux-debug"]), 0);
}
#[test]
fn test_preset_multiple_parent_cache_priority_matches_cmake() {
    let f = Fixture::new();
    let p = f.path().join("CMakePresets.json");
    let presets = json!({"version":6,"configurePresets":[{"name":"first","hidden":true,"generator":"Ninja","binaryDir":"${sourceDir}/build/infra/infra-debug","cacheVariables":{"CMAKE_TOOLCHAIN_FILE":"${sourceDir}/build/infra/infra-debug/conan/conan_toolchain.cmake"}},{"name":"second","hidden":true,"cacheVariables":{"CMAKE_TOOLCHAIN_FILE":"wrong.cmake","OTHER":"kept"}},{"name":"infra-debug","inherits":["first","second"],"cacheVariables":{"CMAKE_BUILD_TYPE":"Debug"}}]});
    fs::write(p, presets.to_string()).expect("presets");
    assert_code(&f.run(&["prepare", "--context", "linux-debug"]), 0);
}
#[cfg(unix)]
#[test]
fn test_consumed_profile_symlink_into_ignored_tree_is_rejected() {
    let f = Fixture::new();
    fs::create_dir_all(f.path().join("build")).expect("build");
    let p = f.path().join(".infra/dependency/profiles/host");
    fs::rename(&p, f.path().join("build/host")).expect("move fixture input");
    std::os::unix::fs::symlink(f.path().join("build/host"), p).expect("profile link");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
}
#[test]
fn test_consumed_lock_in_digest_excluded_tree_is_rejected() {
    let f = Fixture::new();
    fs::create_dir_all(f.path().join("build")).expect("build");
    fs::copy(f.path().join("conan/debug.lock"), f.path().join("build/debug.lock")).expect("ignored lock");
    let p = f.path().join(".infra/dependency/config.toml");
    fs::write(
        &p,
        fs::read_to_string(&p)
            .expect("config")
            .replace("lockfile = 'conan/debug.lock'", "lockfile = 'build/debug.lock'"),
    )
    .expect("config");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
    assert!(f.report()["diagnostics"].to_string().contains("digest"));
}

fn run_with_report(fixture: &Fixture, report: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cpp-infra-dependency"))
        .arg("--project")
        .arg(fixture.path())
        .arg("--report")
        .arg(report)
        .args(args)
        .env(
            "PATH",
            format!(
                "{}:{}",
                fixture.path().join("bin").display(),
                std::env::var("PATH").expect("PATH")
            ),
        )
        .output()
        .expect("run CLI with explicit report")
}
fn report_at(fixture: &Fixture, path: &str) -> Value {
    serde_json::from_slice(&fs::read(fixture.path().join(path)).expect("report exists")).expect("report JSON")
}
#[test]
fn test_root_report_repeat_preserves_source_identity_and_context_metadata() {
    let f = Fixture::new();
    assert_code(
        &run_with_report(&f, "result.json", &["check", "--context", "linux-debug"]),
        0,
    );
    let first = report_at(&f, "result.json");
    assert_eq!(first["source_excluded_paths"], json!(["result.json"]));
    assert_code(
        &run_with_report(
            &f,
            "result.json",
            &[
                "prepare",
                "--context",
                "linux-debug",
                "--output",
                "build/root-context.json",
            ],
        ),
        0,
    );
    let second = report_at(&f, "result.json");
    assert_eq!(second["source_digest"], first["source_digest"]);
    assert_eq!(second["config_digest"], first["config_digest"]);
    let context = report_at(&f, "build/root-context.json");
    assert_eq!(context["source_excluded_paths"], json!(["result.json"]));
    assert_eq!(context["source_digest"], first["source_digest"]);
}
#[test]
fn test_root_report_does_not_overwrite_unknown_business_json() {
    let f = Fixture::new();
    let path = f.path().join("business.json");
    let original = r#"{"orders":[1,2,3]}"#;
    fs::write(&path, original).expect("business input");
    assert_code(
        &run_with_report(&f, "business.json", &["check", "--context", "linux-debug"]),
        2,
    );
    assert_eq!(fs::read_to_string(path).expect("preserved business input"), original);
}
#[test]
fn test_source_exclusions_cannot_hide_real_inputs_or_alias_paths() {
    let f = Fixture::new();
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
    let identity = f.report();
    for candidate in [
        "src/pretend.json",
        ".infra/project.toml",
        "conan/debug.lock",
        "README.json",
        "./business.json",
        "../outside.json",
        "business\\alias.json",
        "",
    ] {
        let context = json!({"schema":1,"run_id":"malicious-exclusion","task_id":"dependency","bundle_id":"development","project":f.path().canonicalize().expect("project"),"profile":"debug","source_digest":identity["source_digest"],"config_digest":identity["config_digest"],"report_root":f.path().join("build/infra-runs/exclusion-tests"),"source_excluded_paths":[candidate]});
        fs::write(f.path().join("build/exclusion-context.json"), context.to_string()).expect("context");
        assert_code(
            &f.run(&[
                "--run-context",
                "build/exclusion-context.json",
                "check",
                "--context",
                "linux-debug",
            ]),
            2,
        );
    }
}
#[test]
fn test_inherited_report_exclusions_are_validated_and_preserved() {
    let f = Fixture::new();
    assert_code(
        &run_with_report(&f, "parent.json", &["check", "--context", "linux-debug"]),
        0,
    );
    let identity = report_at(&f, "parent.json");
    let context = json!({"schema":1,"run_id":"parent-exclusion","task_id":"dependency-child","bundle_id":"development","project":f.path().canonicalize().expect("project"),"profile":"debug","source_digest":identity["source_digest"],"config_digest":identity["config_digest"],"report_root":f.path().join("build/infra-runs/parent-exclusion"),"source_excluded_paths":["parent.json","parent.json"]});
    fs::write(f.path().join("build/exclusion-context.json"), context.to_string()).expect("context");
    assert_code(
        &f.run(&[
            "--run-context",
            "build/exclusion-context.json",
            "check",
            "--context",
            "linux-debug",
        ]),
        0,
    );
    assert_eq!(f.report()["source_excluded_paths"], json!(["parent.json"]));
    assert_eq!(f.report()["source_digest"], identity["source_digest"]);
}
#[test]
fn test_missing_declared_preset_and_profile_inputs_cannot_be_report_outputs() {
    let f = Fixture::new();
    let p = f.path().join("CMakePresets.json");
    let mut presets: Value = serde_json::from_slice(&fs::read(&p).expect("presets")).expect("json");
    presets["include"] = json!(["custom-presets.json"]);
    fs::write(p, presets.to_string()).expect("include");
    assert_code(
        &run_with_report(&f, "custom-presets.json", &["check", "--context", "linux-debug"]),
        2,
    );
    assert!(!f.path().join("custom-presets.json").exists());
    let f = Fixture::new();
    fs::write(
        f.path().join(".infra/dependency/profiles/host"),
        "include(../../../profile-input.json)\n",
    )
    .expect("profile include");
    assert_code(
        &run_with_report(&f, "profile-input.json", &["check", "--context", "linux-debug"]),
        2,
    );
    assert!(!f.path().join("profile-input.json").exists());
}
#[cfg(unix)]
#[test]
fn test_external_report_symlink_cannot_alias_project_input() {
    let f = Fixture::new();
    fs::write(f.path().join("src/input.json"), "{\"source\":true}").expect("source");
    let outside = tempdir().expect("external path");
    std::os::unix::fs::symlink(f.path().join("src"), outside.path().join("alias")).expect("input alias");
    let report = outside.path().join("alias/input.json");
    let original = fs::read(f.path().join("src/input.json")).expect("source bytes");
    assert_code(
        &run_with_report(
            &f,
            report.to_str().expect("path"),
            &["check", "--context", "linux-debug"],
        ),
        2,
    );
    assert_eq!(fs::read(f.path().join("src/input.json")).expect("preserved"), original);
}
#[test]
fn test_recipe_created_business_report_target_is_preserved_on_error() {
    let f = Fixture::new();
    let conan = f.path().join("bin/conan");
    let script=fs::read_to_string(&conan).expect("fake conan").replace("g=json.loads((p/'fixture-graph.json').read_text())","(p/'result.json').write_text('{\"business\":\"created by recipe\"}')\ng=json.loads((p/'fixture-graph.json').read_text())");
    fs::write(conan, script).expect("mutating recipe adapter");
    let output = run_with_report(&f, "result.json", &["check", "--context", "linux-debug"]);
    assert!(!output.status.success());
    assert_eq!(report_at(&f, "result.json"), json!({"business":"created by recipe"}));
}

#[test]
fn test_policy_baseline_rejects_every_digest_excluded_root() {
    for root in [
        ".git",
        "build",
        "target",
        ".infra/.tmp",
        ".infra/transactions",
        ".infra/.transactions",
    ] {
        let f = Fixture::new();
        fs::create_dir_all(f.path().join(root)).expect("excluded fixture directory");
        let target = format!("{root}/excluded-baseline.toml");
        fs::copy(
            f.path().join(".infra/dependency/policy/baseline.toml"),
            f.path().join(&target),
        )
        .expect("excluded baseline fixture");
        let config = f.path().join(".infra/dependency/config.toml");
        fs::write(
            &config,
            fs::read_to_string(&config)
                .expect("config")
                .replace(".infra/dependency/policy/baseline.toml", &target),
        )
        .expect("policy path");
        assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
        assert!(
            f.report()["diagnostics"].to_string().contains("digest"),
            "excluded input {target} did not produce a digest-boundary diagnostic"
        );
    }
}
#[cfg(unix)]
#[test]
fn test_project_dependency_config_and_baseline_cannot_link_into_build() {
    for input in [
        ".infra/project.toml",
        ".infra/dependency/config.toml",
        ".infra/dependency/policy/baseline.toml",
    ] {
        let f = Fixture::new();
        fs::create_dir_all(f.path().join("build")).expect("build directory");
        let target = f.path().join("build/excluded-input.toml");
        fs::rename(f.path().join(input), &target).expect("move disposable fixture input");
        std::os::unix::fs::symlink(&target, f.path().join(input)).expect("input alias");
        assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
        assert!(
            f.report()["diagnostics"].to_string().contains("symlink"),
            "input {input} was not rejected before reading"
        );
    }
}
#[cfg(unix)]
#[test]
fn test_old_context_rejects_baseline_switched_to_excluded_target() {
    let f = Fixture::new();
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
    let identity = f.report();
    let context = json!({"schema":1,"run_id":"policy-boundary","task_id":"dependency","bundle_id":"development","project":f.path().canonicalize().expect("project"),"profile":"debug","source_digest":identity["source_digest"],"config_digest":identity["config_digest"],"report_root":f.path().join("build/infra-runs/policy-boundary")});
    fs::write(f.path().join("build/policy-context.json"), context.to_string()).expect("context");
    let baseline = f.path().join(".infra/dependency/policy/baseline.toml");
    let excluded = f.path().join("build/excluded-baseline.toml");
    fs::rename(&baseline, &excluded).expect("move fixture baseline");
    std::os::unix::fs::symlink(&excluded, &baseline).expect("excluded baseline alias");
    let modified = fs::read_to_string(&excluded)
        .expect("baseline")
        .replace(">=11.0.0,<12.0.0", ">=1.0.0,<99.0.0");
    fs::write(excluded, modified).expect("change excluded policy");
    assert_code(
        &f.run(&[
            "--run-context",
            "build/policy-context.json",
            "check",
            "--context",
            "linux-debug",
        ]),
        2,
    );
    assert!(f.report()["diagnostics"].to_string().contains("stale"));
}
#[cfg(unix)]
#[test]
fn test_fresh_check_rejects_recipe_replacing_baseline_with_excluded_link() {
    let f = Fixture::new();
    let adapter = f.path().join("bin/conan");
    let script=fs::read_to_string(&adapter).expect("adapter").replace("g=json.loads((p/'fixture-graph.json').read_text())","g=json.loads((p/'fixture-graph.json').read_text())\noriginal=p/'.infra/dependency/policy/baseline.toml'\ntarget=p/'build/replaced-baseline.toml'\noriginal.rename(target)\noriginal.symlink_to(target)");
    fs::write(adapter, script).expect("fixture recipe mutation");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 2);
    assert!(f.report()["diagnostics"].to_string().contains("changed"));
}
#[test]
fn test_nested_build_named_policy_directory_remains_tracked_and_accepted() {
    let f = Fixture::new();
    let directory = f.path().join(".infra/dependency/policy/build");
    fs::create_dir_all(&directory).expect("ordinary nested directory");
    fs::copy(
        f.path().join(".infra/dependency/policy/baseline.toml"),
        directory.join("baseline.toml"),
    )
    .expect("tracked nested policy");
    let config = f.path().join(".infra/dependency/config.toml");
    fs::write(
        &config,
        fs::read_to_string(&config)
            .expect("config")
            .replace("policy/baseline.toml", "policy/build/baseline.toml"),
    )
    .expect("nested policy path");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
}
#[cfg(unix)]
#[test]
fn test_root_report_preflight_validates_config_location_before_parsing() {
    for input in [".infra/project.toml", ".infra/dependency/config.toml"] {
        let f = Fixture::new();
        fs::create_dir_all(f.path().join("build")).expect("build");
        let excluded = f.path().join("build/untracked-config.toml");
        fs::rename(f.path().join(input), &excluded).expect("move disposable fixture config");
        fs::write(&excluded, "this is not valid TOML ???").expect("untracked config contents");
        std::os::unix::fs::symlink(&excluded, f.path().join(input)).expect("excluded config link");
        let output = run_with_report(&f, "result.json", &["check", "--context", "linux-debug"]);
        assert_code(&output, 2);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("symlink"),
            "untracked config was parsed before validating its location: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!f.path().join("result.json").exists());
    }
}
#[cfg(unix)]
#[test]
fn test_root_report_preflight_rejects_original_preset_and_profile_aliases() {
    let f = Fixture::new();
    fs::create_dir_all(f.path().join("build")).expect("build");
    fs::create_dir_all(f.path().join("configs")).expect("tracked inputs");
    fs::write(f.path().join("configs/input.json"), "not valid JSON").expect("invalid tracked JSON");
    std::os::unix::fs::symlink(f.path().join("configs/input.json"), f.path().join("build/alias.json"))
        .expect("ignored preset alias");
    let presets = f.path().join("CMakePresets.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&presets).expect("presets")).expect("JSON");
    value["include"] = json!(["build/alias.json"]);
    fs::write(presets, value.to_string()).expect("preset include");
    let output = run_with_report(&f, "result.json", &["check", "--context", "linux-debug"]);
    assert_code(&output, 2);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("symlink"),
        "preset alias target was read first: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let f = Fixture::new();
    fs::create_dir_all(f.path().join("build")).expect("build");
    fs::create_dir_all(f.path().join("configs")).expect("tracked inputs");
    fs::write(f.path().join("configs/profile"), [0xff]).expect("unreadable profile fixture");
    std::os::unix::fs::symlink(f.path().join("configs/profile"), f.path().join("build/alias.profile"))
        .expect("ignored profile alias");
    fs::write(
        f.path().join(".infra/dependency/profiles/host"),
        "include(../../../build/alias.profile)\n",
    )
    .expect("profile include");
    let output = run_with_report(&f, "result.json", &["check", "--context", "linux-debug"]);
    assert_code(&output, 2);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("symlink"),
        "profile alias target was read first: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_prepare_output_rejects_project_inputs_and_unknown_business_files_before_install() {
    for target in [
        "conanfile.txt",
        "CMakePresets.json",
        ".infra/dependency/config.toml",
        "conan/debug.lock",
        "src/input.json",
        "business.json",
        "build/business.json",
    ] {
        let f = Fixture::new();
        if target.ends_with("input.json") || target.ends_with("business.json") {
            let path = f.path().join(target);
            fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
            fs::write(path, "{\"business\":42}").expect("business fixture");
        }
        let original = fs::read(f.path().join(target)).expect("protected input");
        let output = f.run(&["prepare", "--context", "linux-debug", "--output", target]);
        assert_code(&output, 2);
        assert_eq!(
            fs::read(f.path().join(target)).expect("preserved input"),
            original,
            "prepare overwrote {target}"
        );
        assert!(
            !f.path().join("build/infra/infra-debug").exists(),
            "invalid output reached install for {target}"
        );
    }
}
#[cfg(unix)]
#[test]
fn test_prepare_output_symlink_cannot_alias_source_input() {
    let f = Fixture::new();
    fs::create_dir_all(f.path().join("build")).expect("build");
    let source = f.path().join("src/input.json");
    fs::write(&source, "{\"source\":true}").expect("source");
    let original = fs::read(&source).expect("original");
    std::os::unix::fs::symlink(&source, f.path().join("build/context.json")).expect("output alias");
    assert_code(
        &f.run(&["prepare", "--context", "linux-debug", "--output", "build/context.json"]),
        2,
    );
    assert_eq!(fs::read(source).expect("preserved source"), original);
    assert!(!f.path().join("build/infra/infra-debug").exists());
}
#[test]
fn test_prepare_revalidates_output_before_persisting_context() {
    let f = Fixture::new();
    let adapter = f.path().join("bin/conan");
    let script = fs::read_to_string(&adapter).expect("adapter").replace(
        "if a and a[0]=='install':",
        "if a and a[0]=='install':\n (p/'build/context.json').write_text('{\"business\":\"created during install\"}')",
    );
    fs::write(adapter, script).expect("install-side fixture mutation");
    assert_code(
        &f.run(&["prepare", "--context", "linux-debug", "--output", "build/context.json"]),
        2,
    );
    assert_eq!(
        report_at(&f, "build/context.json"),
        json!({"business":"created during install"})
    );
}
#[test]
fn test_sync_staging_keeps_nested_build_target_profiles_and_omits_root_caches() {
    let f = Fixture::new();
    for directory in [
        "config/build",
        "config/target",
        ".infra/.tmp",
        ".infra/transactions",
        ".infra/.transactions",
    ] {
        fs::create_dir_all(f.path().join(directory)).expect("fixture directory");
    }
    fs::copy(
        f.path().join(".infra/dependency/profiles/host"),
        f.path().join("config/target/common"),
    )
    .expect("shared profile");
    fs::write(f.path().join("config/build/profile"), "include(../target/common)\n").expect("nested profile");
    let config = f.path().join(".infra/dependency/config.toml");
    fs::write(
        &config,
        fs::read_to_string(&config)
            .expect("config")
            .replace(".infra/dependency/profiles/build", "config/build/profile")
            .replace(".infra/dependency/profiles/host", "config/build/profile"),
    )
    .expect("nested config");
    let recipe = f.path().join("conanfile.txt");
    let original = fs::read_to_string(&recipe)
        .expect("recipe")
        .replace("fmt/11.0.2", "fmt/10.0.0");
    fs::write(&recipe, &original).expect("outdated recipe");
    let adapter = f.path().join("bin/conan");
    let script=fs::read_to_string(&adapter).expect("adapter").replace("g=json.loads((p/'fixture-graph.json').read_text())","for option in ['--profile:build','--profile:host']:\n profile=pathlib.Path(a[a.index(option)+1])\n if not profile.is_file(): print('missing staged profile',file=sys.stderr); sys.exit(1)\n included=profile.parent/'../target/common'\n if not included.is_file(): print('missing staged include',file=sys.stderr); sys.exit(1)\nfor excluded in ['.infra/.tmp','.infra/transactions','.infra/.transactions']:\n if (p/excluded).exists(): print('copied transaction cache',file=sys.stderr); sys.exit(1)\ng=json.loads((p/'fixture-graph.json').read_text())");
    fs::write(adapter, script).expect("profile-reading fixture");
    assert_code(&f.run(&["sync", "--dry-run"]), 0);
    assert_eq!(fs::read_to_string(&recipe).expect("dry-run original"), original);
    assert_code(&f.run(&["sync"]), 0);
    assert!(
        fs::read_to_string(recipe)
            .expect("synced recipe")
            .contains("fmt/11.0.2")
    );
}
#[test]
fn test_source_exclusion_original_text_rejects_separator_and_drive_aliases() {
    let f = Fixture::new();
    fs::write(f.path().join("business.json"), "{\"business\":true}").expect("business input");
    assert_code(&f.run(&["check", "--context", "linux-debug"]), 0);
    let identity = f.report();
    for relative in [
        "business.json/",
        "business.json//",
        "reports//result.json",
        "reports/./result.json",
        "/result.json",
        "C:/result.json",
        "C:result.json",
        "reports\\result.json",
    ] {
        let context = json!({"schema":1,"run_id":"malformed-original","task_id":"dependency","bundle_id":"development","project":f.path().canonicalize().expect("project"),"profile":"debug","source_digest":identity["source_digest"],"config_digest":identity["config_digest"],"report_root":f.path().join("build/infra-runs/malformed-original"),"source_excluded_paths":[relative]});
        fs::write(f.path().join("build/malformed-context.json"), context.to_string()).expect("context");
        assert_code(
            &f.run(&[
                "--run-context",
                "build/malformed-context.json",
                "check",
                "--context",
                "linux-debug",
            ]),
            2,
        );
        assert_eq!(report_at(&f, "business.json"), json!({"business":true}));
    }
}
#[test]
fn test_prepare_report_artifacts_reference_final_graph_bytes() {
    use sha2::Digest;
    use sha2::Sha256;
    let f = Fixture::new();
    let adapter = f.path().join("bin/conan");
    let script = fs::read_to_string(&adapter).expect("adapter").replace(
        "print(json.dumps(g))",
        "if a[:2]==['graph','info']: g['graph']['nodes']['0']['generators_folder']=None\nprint(json.dumps(g))",
    );
    fs::write(adapter, script).expect("graph-info and install distinction");
    assert_code(&f.run(&["prepare", "--context", "linux-debug"]), 0);
    let report = f.report();
    let root = std::path::PathBuf::from(report["report_root"].as_str().expect("report root"));
    for artifact in report["artifacts"].as_array().expect("artifacts") {
        let path = root.join(artifact["path"].as_str().expect("artifact path"));
        assert_eq!(
            artifact["sha256"],
            Sha256::digest(fs::read(path).expect("artifact bytes"))
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            "artifact metadata became stale: {}",
            artifact["path"]
        );
    }
}
#[test]
fn test_prepare_refreshes_its_own_existing_context_artifact() {
    let f = Fixture::new();
    assert_code(
        &f.run(&[
            "prepare",
            "--context",
            "linux-debug",
            "--output",
            "build/reused-context.json",
        ]),
        0,
    );
    assert_eq!(report_at(&f, "build/reused-context.json")["baseline_id"], "test-1");
    let policy = f.path().join(".infra/dependency/policy/baseline.toml");
    fs::write(
        &policy,
        fs::read_to_string(&policy).expect("policy").replace("test-1", "test-2"),
    )
    .expect("updated baseline identity");
    assert_code(
        &f.run(&[
            "prepare",
            "--context",
            "linux-debug",
            "--output",
            "build/reused-context.json",
        ]),
        0,
    );
    assert_eq!(report_at(&f, "build/reused-context.json")["baseline_id"], "test-2");
}
