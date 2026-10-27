// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

//! Explicit opt-in acceptance tests against an installed Conan 2 executable.
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;

use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use tempfile::tempdir;

struct Real {
    root: TempDir,
    conan: PathBuf,
}
impl Real {
    fn new() -> Self {
        let conan = PathBuf::from(
            std::env::var_os("CONAN_TEST_EXECUTABLE").expect("set CONAN_TEST_EXECUTABLE for explicit real acceptance"),
        );
        assert!(conan.is_file(), "Conan executable missing");
        let root = tempdir().expect("fixture");
        let p = root.path();
        for d in [
            ".infra/dependency/policy",
            ".infra/dependency/profiles",
            "src",
            "conan",
            "bin",
            "build",
        ] {
            fs::create_dir_all(p.join(d)).expect("dir");
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&conan, p.join("bin/conan")).expect("Conan symlink");
        fs::write(p.join(".infra/project.toml"),"schema=1\nkind='library'\nsource_roots=['src']\ntest_roots=[]\ngenerated_roots=[]\nthird_party_roots=[]\n[profiles.debug]\nconfigure='infra-debug'\nbuild='infra-debug'\ntest='infra-debug'\nconan_context='local-debug'\n").expect("project");
        fs::write(p.join(".infra/dependency/config.toml"),"schema=1\nbackend='conan2'\nrecipe='conanfile.txt'\npolicy='.infra/dependency/policy/baseline.toml'\n[contexts.local-debug]\nbuild_profile='.infra/dependency/profiles/default'\nhost_profile='.infra/dependency/profiles/default'\nlockfile='conan/debug.lock'\nprofile='debug'\ngenerators_dir='build/infra/infra-debug/conan'\nbuild_missing=false\nremotes=[]\n").expect("config");
        fs::write(p.join(".infra/dependency/policy/baseline.toml"),"schema=1\nbaseline_id='real-local'\ninternal_patterns=[]\nallow_refs=[]\n[direct]\nlocal-header='local-header/1.0.0'\n[resolved]\nlocal-header='>=1.0.0,<2.0.0'\n").expect("policy");
        fs::write(p.join(".infra/dependency/profiles/default"),"[settings]\nos=Linux\narch=x86_64\ncompiler=gcc\ncompiler.version=13\ncompiler.libcxx=libstdc++11\ncompiler.cppstd=20\nbuild_type=Debug\n").expect("profile");
        fs::write(p.join("CMakePresets.json"),json!({"version":6,"configurePresets":[{"name":"infra-debug","generator":"Ninja","binaryDir":"${sourceDir}/build/infra/infra-debug","toolchainFile":"${sourceDir}/build/infra/infra-debug/conan/conan_toolchain.cmake"}]}).to_string()).expect("presets");
        fs::write(p.join("conanfile.txt"), "[generators]\nCMakeDeps\nCMakeToolchain\n").expect("recipe");
        Self { root, conan }
    }
    fn path(&self) -> &Path {
        self.root.path()
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cpp-infra-dependency"))
            .arg("--project")
            .arg(self.path())
            .args(["--report", "build/report.json", "--offline"])
            .args(args)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.path().join("bin").display(),
                    std::env::var("PATH").expect("PATH")
                ),
            )
            .env("CONAN_HOME", self.path().join("build/conan-home"))
            .output()
            .expect("CLI")
    }
    fn assert_success(&self, args: &[&str]) {
        let o = self.run(args);
        assert!(
            o.status.success(),
            "args={args:?}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
    }
}
#[test]
#[ignore = "requires explicit CONAN_TEST_EXECUTABLE; no network or user Conan home is used"]
fn test_real_conan_empty_txt_lock_inventory_and_prepare() {
    let f = Real::new();
    f.assert_success(&["lock", "update", "--context", "local-debug"]);
    f.assert_success(&["check", "--context", "local-debug"]);
    f.assert_success(&["inventory", "--context", "local-debug"]);
    f.assert_success(&["prepare", "--context", "local-debug", "--output", "build/context.json"]);
    assert!(
        f.path()
            .join("build/infra/infra-debug/conan/conan_toolchain.cmake")
            .is_file()
    );
    assert!(!f.path().join("CMakeUserPresets.json").exists());
    let c: Value =
        serde_json::from_slice(&fs::read(f.path().join("build/context.json")).expect("context")).expect("json");
    assert_eq!(c["settings"]["compiler"], "gcc");
    assert_eq!(c["settings"]["build_type"], "Debug");
}
#[test]
#[ignore = "requires explicit CONAN_TEST_EXECUTABLE; creates a local package in an isolated cache"]
fn test_real_conan_python_recipe_local_header_package() {
    let f = Real::new();
    let package = f.path().join("build/package");
    fs::create_dir_all(&package).expect("package dir");
    fs::write(package.join("conanfile.py"),"from conan import ConanFile\nimport os\nclass LocalHeader(ConanFile):\n    name = 'local-header'\n    version = '1.0.0'\n    package_type = 'header-library'\n    def package(self):\n        os.makedirs(os.path.join(self.package_folder, 'include'), exist_ok=True)\n        with open(os.path.join(self.package_folder, 'include', 'local.hpp'), 'w') as f:\n            f.write('#pragma once\\ninline int local_value() { return 42; }\\n')\n").expect("local recipe");
    let profile = f.path().join(".infra/dependency/profiles/default");
    let o = Command::new(&f.conan)
        .args(["create"])
        .arg(&package)
        .arg("--profile:build")
        .arg(&profile)
        .arg("--profile:host")
        .arg(&profile)
        .arg("--no-remote")
        .env("CONAN_HOME", f.path().join("build/conan-home"))
        .output()
        .expect("conan create");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let config = f.path().join(".infra/dependency/config.toml");
    fs::write(
        &config,
        fs::read_to_string(&config)
            .expect("config")
            .replace("conanfile.txt", "conanfile.py"),
    )
    .expect("python config");
    fs::write(f.path().join("conanfile.py"),"from conan import ConanFile\nclass Consumer(ConanFile):\n    settings = 'os', 'arch', 'compiler', 'build_type'\n    requires = 'local-header/1.0.0'\n    generators = 'CMakeDeps', 'CMakeToolchain'\n").expect("python consumer");
    f.assert_success(&["lock", "update", "--context", "local-debug"]);
    f.assert_success(&["check", "--context", "local-debug"]);
    f.assert_success(&["prepare", "--context", "local-debug", "--output", "build/context.json"]);
    let r: Value =
        serde_json::from_slice(&fs::read(f.path().join("build/report.json")).expect("report")).expect("json");
    let node = &r["data"]["inventory"]["nodes"][0];
    assert_eq!(node["name"], "local-header");
    assert!(node["recipe_revision"].as_str().is_some());
    assert!(node["package_id"].as_str().is_some());
    assert!(node["package_revision"].as_str().is_some());
}
#[test]
#[ignore = "requires explicit Conan; verifies Conan-home profile shadowing is blocked"]
fn test_real_conan_home_profile_cannot_shadow_project_bare_include() {
    let f = Real::new();
    let profile = f.path().join(".infra/dependency/profiles/default");
    let content = fs::read_to_string(&profile).expect("profile");
    fs::write(f.path().join(".infra/dependency/profiles/common"), &content).expect("local common");
    fs::write(&profile, "include(common)\n").expect("bare include");
    fs::create_dir_all(f.path().join("build/conan-home/profiles")).expect("Conan profiles");
    fs::write(
        f.path().join("build/conan-home/profiles/common"),
        content.replace("build_type=Debug", "build_type=Release"),
    )
    .expect("shadow profile");
    let o = f.run(&["lock", "update", "--context", "local-debug"]);
    assert_eq!(
        o.status.code(),
        Some(2),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("include"));
}
