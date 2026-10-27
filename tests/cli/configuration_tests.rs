// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

use std::fs;

use serde_json::Value;
use serde_json::json;

use crate::Fixture;
use crate::assert_code;

fn replace_input(fixture: &Fixture, input: &str, from: &str, to: &str) {
    let path = fixture.path().join(input);
    let original = fs::read_to_string(&path).expect("read fixture input");
    assert!(original.contains(from), "replacement target missing in {input}: {from}");
    fs::write(path, original.replace(from, to)).expect("replace fixture input");
}

fn assert_configuration_error(fixture: &Fixture, args: &[&str], diagnostic: &str) {
    let output = fixture.run(args);
    assert_code(&output, 2);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(diagnostic), "expected {diagnostic:?}, got {stderr}");
}

fn write_presets(fixture: &Fixture, presets: &Value) {
    fs::write(fixture.path().join("CMakePresets.json"), presets.to_string()).expect("write presets");
}

#[test]
fn test_dependency_configuration_rejects_invalid_schema_backend_timeout_and_unknown_fields() {
    for (from, to, diagnostic) in [
        ("schema = 1", "schema = 2", "expected schema=1, backend=conan2"),
        (
            "backend = 'conan2'",
            "backend = 'conan1'",
            "expected schema=1, backend=conan2",
        ),
        ("schema = 1", "schema = 1\ntimeout_seconds = 0", "positive timeout"),
        (
            "schema = 1",
            "schema = 1\nunrecognized = true",
            "unknown field `unrecognized`",
        ),
        (
            "build_missing = false",
            "build_missing = false\nunrecognized = true",
            "unknown field `unrecognized`",
        ),
    ] {
        let fixture = Fixture::new();
        replace_input(&fixture, ".infra/dependency/config.toml", from, to);
        assert_configuration_error(&fixture, &["plan", "--context", "linux-debug"], diagnostic);
    }
    let fixture = Fixture::new();
    fs::write(
        fixture.path().join(".infra/dependency/config.toml"),
        "schema = 1\nbackend = 'conan2'\nrecipe = 'conanfile.txt'\npolicy = '.infra/dependency/policy/baseline.toml'\ncontexts = {}\n",
    )
    .expect("empty contexts");
    assert_configuration_error(&fixture, &["plan", "--context", "linux-debug"], "nonempty contexts");
}

#[test]
fn test_project_configuration_rejects_invalid_schema_kind_roots_and_profile_bindings() {
    for (from, to, diagnostic) in [
        ("schema = 1", "schema = 2", "unsupported project schema/kind"),
        (
            "kind = 'library'",
            "kind = 'unknown'",
            "unsupported project schema/kind",
        ),
        (
            "schema = 1",
            "schema = 1\nunrecognized = true",
            "unknown field `unrecognized`",
        ),
        (
            "source_roots = ['src']",
            "source_roots = ['missing-source']",
            "configured root is not a directory: missing-source",
        ),
        (
            "test_roots = []",
            "test_roots = ['src']",
            "overlapping project roots: src",
        ),
        (
            "build = 'infra-debug'",
            "build = ''",
            "profile debug must bind conan_context=linux-debug",
        ),
        (
            "test = 'infra-debug'",
            "test = ''",
            "profile debug must bind conan_context=linux-debug",
        ),
        (
            "conan_context = 'linux-debug'",
            "conan_context = 'other'",
            "profile debug must bind conan_context=linux-debug",
        ),
        (
            "conan_context = 'linux-debug'",
            "",
            "profile debug must bind conan_context=linux-debug",
        ),
        (
            "configure = 'infra-debug'",
            "configure = 'infra-debug'\nunrecognized = true",
            "unknown field `unrecognized`",
        ),
    ] {
        let fixture = Fixture::new();
        replace_input(&fixture, ".infra/project.toml", from, to);
        assert_configuration_error(&fixture, &["plan", "--context", "linux-debug"], diagnostic);
    }
}

#[test]
fn test_project_root_overlap_detects_both_parent_and_child_orderings() {
    for roots in ["['src', 'src/nested']", "['src/nested', 'src']"] {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.path().join("src/nested")).expect("nested source root");
        replace_input(&fixture, ".infra/project.toml", "['src']", roots);
        assert_configuration_error(
            &fixture,
            &["plan", "--context", "linux-debug"],
            "overlapping project roots:",
        );
    }
}

#[test]
fn test_context_configuration_rejects_unknown_profile_and_reverse_binding() {
    let fixture = Fixture::new();
    replace_input(
        &fixture,
        ".infra/dependency/config.toml",
        "profile = 'debug'",
        "profile = 'missing'",
    );
    assert_configuration_error(
        &fixture,
        &["plan", "--context", "linux-debug"],
        "unknown project profile: missing",
    );
    for context in ["linux-debug", "missing"] {
        let fixture = Fixture::new();
        let path = fixture.path().join(".infra/project.toml");
        let project = fs::read_to_string(&path).expect("project configuration");
        fs::write(path, format!("{project}\n[profiles.orphan]\nconfigure = 'infra-debug'\nbuild = 'infra-debug'\nconan_context = '{context}'\n"))
            .expect("orphan project profile");
        assert_configuration_error(
            &fixture,
            &["plan", "--context", "linux-debug"],
            "project profile orphan has inconsistent Conan context",
        );
    }
    let fixture = Fixture::new();
    assert_configuration_error(
        &fixture,
        &["plan", "--context", "missing"],
        "unknown Conan context: missing",
    );
}

#[test]
fn test_context_identifier_rejects_empty_dot_traversal_and_non_ascii_names() {
    for name in ["", ".", "..", "linux/debug", "linux debug", "调试"] {
        let fixture = Fixture::new();
        replace_input(
            &fixture,
            ".infra/dependency/config.toml",
            "[contexts.linux-debug]",
            &format!("[contexts.'{name}']"),
        );
        assert_configuration_error(
            &fixture,
            &["plan", "--context", "linux-debug"],
            "empty context identifier",
        );
    }
}

#[test]
fn test_generators_directory_must_be_a_strict_binary_directory_descendant() {
    for generators in ["build/infra/infra-debug", "build/infra/other/conan", "src/conan"] {
        let fixture = Fixture::new();
        replace_input(
            &fixture,
            ".infra/dependency/config.toml",
            "build/infra/infra-debug/conan",
            generators,
        );
        assert_configuration_error(
            &fixture,
            &["plan", "--context", "linux-debug"],
            "generators_dir must be within",
        );
    }
}

#[test]
fn test_recipe_filename_and_remote_registration_are_validated_before_conan() {
    let fixture = Fixture::new();
    fs::write(fixture.path().join("recipe.txt"), "[requires]\nfmt/11.0.2\n").expect("wrong recipe filename");
    replace_input(&fixture, ".infra/dependency/config.toml", "conanfile.txt", "recipe.txt");
    assert_configuration_error(
        &fixture,
        &["plan", "--context", "linux-debug"],
        "recipe must be an existing conanfile.txt or conanfile.py",
    );
    for remotes in ["['missing']", "['central', 'central']"] {
        let fixture = Fixture::new();
        replace_input(
            &fixture,
            ".infra/dependency/config.toml",
            "remotes = []",
            &format!("remotes = {remotes}"),
        );
        let path = fixture.path().join(".infra/dependency/policy/baseline.toml");
        let policy = fs::read_to_string(&path).expect("policy");
        fs::write(
            path,
            format!("{policy}\n[remotes]\ncentral = 'https://example.invalid/conan'\n"),
        )
        .expect("registered remote");
        assert_configuration_error(
            &fixture,
            &["plan", "--context", "linux-debug"],
            "remote is not uniquely registered by policy:",
        );
    }
}

#[test]
fn test_profile_include_cycle_and_dynamic_syntax_are_rejected() {
    for profile in [
        "include(./host)\n",
        "include './common'\n",
        "{{ compiler }}\n",
        "{% if target %}\n",
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.path().join(".infra/dependency/profiles/host"), profile).expect("invalid profile");
        let diagnostic = if profile == "include(./host)\n" {
            "Conan profile include cycle"
        } else {
            "dynamic or malformed profile includes are unsupported"
        };
        assert_configuration_error(&fixture, &["plan", "--context", "linux-debug"], diagnostic);
    }
}

#[test]
fn test_profile_includes_accept_quoted_relative_absolute_and_shared_inputs() {
    let fixture = Fixture::new();
    let directory = fixture.path().join(".infra/dependency/profiles");
    fs::write(directory.join("common"), "[settings]\nbuild_type=Debug\n").expect("common profile");
    fs::write(directory.join("child"), "include('./common')\n").expect("child profile");
    fs::write(directory.join("host"), "include(\"./common\")\ninclude(./child)\n").expect("shared includes");
    fs::write(
        directory.join("build"),
        format!("include({})\n", directory.join("common").display()),
    )
    .expect("absolute profile include");
    assert_code(&fixture.run(&["plan", "--context", "linux-debug"]), 0);
    assert!(fixture.report()["commands"].as_array().expect("commands").is_empty());
}

#[test]
fn test_baseline_configuration_rejects_invalid_identity_references_ranges_and_patterns() {
    for (from, to, diagnostic) in [
        ("schema = 1", "schema = 2", "baseline requires schema=1 and baseline_id"),
        (
            "baseline_id = 'test-1'",
            "baseline_id = ''",
            "baseline requires schema=1 and baseline_id",
        ),
        (
            "schema = 1",
            "schema = 1\nunrecognized = true",
            "unknown field `unrecognized`",
        ),
        (
            "fmt = 'fmt/11.0.2'",
            "other = 'fmt/11.0.2'",
            "baseline direct key differs from require name: other",
        ),
        ("allow_refs = []", "allow_refs = ['invalid']", "invalid Conan reference"),
        (
            ">=11.0.0,<12.0.0",
            ">=11.0",
            "resolved range accepts only numeric three-part comparisons",
        ),
        (
            ">=11.0.0,<12.0.0",
            ">=11.00.0",
            "resolved range accepts only numeric three-part comparisons",
        ),
        (
            ">=11.0.0,<12.0.0",
            ">=11.0.0,",
            "resolved range accepts only numeric three-part comparisons",
        ),
        ("['qubit-*']", "['']", "unsupported internal name pattern"),
        ("['qubit-*']", "['qubit-**']", "unsupported internal name pattern"),
        ("['qubit-*']", "['qubit-?']", "unsupported internal name pattern"),
        ("['qubit-*']", "['qubit-[a]']", "unsupported internal name pattern"),
    ] {
        let fixture = Fixture::new();
        replace_input(&fixture, ".infra/dependency/policy/baseline.toml", from, to);
        assert_configuration_error(&fixture, &["plan", "--context", "linux-debug"], diagnostic);
    }
}

#[test]
fn test_baseline_remote_urls_reject_credentials_query_fragment_and_non_http_schemes() {
    for (name, url) in [
        ("", "https://example.invalid/conan"),
        ("central", "file:///tmp/conan"),
        ("central", "https://user:password@example.invalid/conan"),
        ("central", "https://example.invalid/conan?token=secret"),
        ("central", "https://example.invalid/conan#fragment"),
    ] {
        let fixture = Fixture::new();
        let path = fixture.path().join(".infra/dependency/policy/baseline.toml");
        let policy = fs::read_to_string(&path).expect("baseline");
        fs::write(path, format!("{policy}\n[remotes]\n'{name}' = '{url}'\n")).expect("invalid remote policy");
        assert_configuration_error(
            &fixture,
            &["plan", "--context", "linux-debug"],
            "needs an HTTP(S) URL without credentials/query/fragment",
        );
    }
}

#[test]
fn test_resolved_range_comparisons_accept_and_reject_boundary_versions() {
    for (range, code) in [
        (">=11.0.2", 0),
        (">=11.0.3", 1),
        ("<=11.0.2", 0),
        ("<=11.0.1", 1),
        (">11.0.1", 0),
        (">11.0.2", 1),
        ("<11.0.3", 0),
        ("<11.0.2", 1),
        ("=11.0.2", 0),
        ("=11.0.1", 1),
        ("==11.0.2", 0),
        ("==11.0.3", 1),
        ("11.0.2", 0),
        ("11.0.3", 1),
        (" >= 11.0.2 , < 12.0.0 ", 0),
    ] {
        let fixture = Fixture::new();
        replace_input(
            &fixture,
            ".infra/dependency/policy/baseline.toml",
            ">=11.0.0,<12.0.0",
            range,
        );
        assert_code(&fixture.run(&["check", "--context", "linux-debug"]), code);
        if code == 1 {
            assert!(
                fixture.report()["diagnostics"]
                    .to_string()
                    .contains(&format!("violates {range}"))
            );
        } else {
            assert_eq!(fixture.report()["status"], "passed");
        }
    }
}

#[test]
fn test_preset_schema_include_and_configure_shapes_are_strict() {
    for (presets, diagnostic) in [
        (json!({"version":5}), "only CMake Presets schema 6 is supported"),
        (
            json!({"version":6,"include":"other.json"}),
            "preset include must be array",
        ),
        (json!({"version":6,"include":[42]}), "preset include must be string"),
        (
            json!({"version":6,"configurePresets":{}}),
            "configurePresets must be array",
        ),
        (json!({"version":6,"configurePresets":[{}]}), "preset missing name"),
        (
            json!({"version":6,"configurePresets":[{"name":"infra-debug"},{"name":"infra-debug"}]}),
            "duplicate configure preset: infra-debug",
        ),
        (json!({"version":6}), "unknown configure preset: infra-debug"),
        (
            json!({"version":6,"configurePresets":[{"name":"infra-debug"}]}),
            "preset infra-debug has no binaryDir",
        ),
        (
            json!({"version":6,"configurePresets":[{"name":"infra-debug","inherits":42}]}),
            "invalid preset inherits",
        ),
        (
            json!({"version":6,"configurePresets":[{"name":"infra-debug","inherits":[42]}]}),
            "invalid inherited preset name",
        ),
        (
            json!({"version":6,"configurePresets":[{"name":"infra-debug","inherits":"missing"}]}),
            "unknown configure preset: missing",
        ),
        (
            json!({"version":6,"configurePresets":[{"name":"infra-debug","inherits":"parent"},{"name":"parent","inherits":"infra-debug"}]}),
            "preset inheritance cycle",
        ),
        (
            json!({"version":6,"include":["CMakePresets.json"]}),
            "preset include escapes project or is cyclic",
        ),
    ] {
        let fixture = Fixture::new();
        write_presets(&fixture, &presets);
        assert_configuration_error(&fixture, &["plan", "--context", "linux-debug"], diagnostic);
    }
    let fixture = Fixture::new();
    fs::write(fixture.path().join("CMakePresets.json"), "{ invalid JSON").expect("malformed presets");
    assert_configuration_error(&fixture, &["plan", "--context", "linux-debug"], "preset JSON:");
}

#[test]
fn test_binary_directory_rejects_unsupported_macros_and_non_owned_locations() {
    for (binary, diagnostic) in [
        (
            "${sourceDir}/build/infra/${hostSystemName}",
            "unsupported binaryDir macro",
        ),
        ("${sourceDir}/build/infra", "binaryDir must be under build/infra"),
        ("${sourceDir}/build/other/debug", "binaryDir must be under build/infra"),
        ("${sourceDir}/src/debug", "binaryDir must be under build/infra"),
    ] {
        let fixture = Fixture::new();
        write_presets(
            &fixture,
            &json!({"version":6,"configurePresets":[{"name":"infra-debug","binaryDir":binary}]}),
        );
        assert_configuration_error(&fixture, &["plan", "--context", "linux-debug"], diagnostic);
    }
}

#[test]
fn test_prepare_toolchain_binding_rejects_missing_macro_and_incorrect_paths() {
    for (fields, diagnostic) in [
        (
            json!({}),
            "selected configure preset must explicitly bind the Conan toolchain",
        ),
        (
            json!({"cacheVariables":{"CMAKE_TOOLCHAIN_FILE":{"value":42}}}),
            "selected configure preset must explicitly bind the Conan toolchain",
        ),
        (
            json!({"toolchainFile":"$env{CONAN_TOOLCHAIN}"}),
            "unsupported toolchain path macro",
        ),
        (
            json!({"toolchainFile":"conan/incorrect.cmake"}),
            "configure preset toolchain must be",
        ),
    ] {
        let fixture = Fixture::new();
        let mut preset = json!({"name":"infra-debug","binaryDir":"${sourceDir}/build/infra/${presetName}"});
        preset
            .as_object_mut()
            .expect("preset fields")
            .extend(fields.as_object().expect("toolchain fields").clone());
        write_presets(&fixture, &json!({"version":6,"configurePresets":[preset]}));
        assert_configuration_error(&fixture, &["prepare", "--context", "linux-debug"], diagnostic);
        assert!(
            !fixture
                .path()
                .join("build/infra/infra-debug/conan/conan_toolchain.cmake")
                .exists()
        );
    }
}

#[test]
fn test_prepare_accepts_cache_variable_value_and_source_or_binary_relative_toolchain_paths() {
    for fields in [
        json!({"cacheVariables":{"CMAKE_TOOLCHAIN_FILE":{"type":"FILEPATH","value":"${sourceDir}/build/infra/${presetName}/conan/conan_toolchain.cmake"}}}),
        json!({"cacheVariables":{"CMAKE_TOOLCHAIN_FILE":"build/infra/infra-debug/conan/conan_toolchain.cmake"}}),
        json!({"toolchainFile":"conan/conan_toolchain.cmake"}),
    ] {
        let fixture = Fixture::new();
        let mut preset = json!({"name":"infra-debug","binaryDir":"${sourceDir}/build/infra/${presetName}"});
        preset
            .as_object_mut()
            .expect("preset fields")
            .extend(fields.as_object().expect("toolchain fields").clone());
        write_presets(&fixture, &json!({"version":6,"configurePresets":[preset]}));
        assert_code(&fixture.run(&["prepare", "--context", "linux-debug"]), 0);
        assert_eq!(fixture.report()["data"]["dependency_context"]["context"], "linux-debug");
        assert!(
            fixture
                .path()
                .join("build/infra/infra-debug/conan/conan_toolchain.cmake")
                .is_file()
        );
    }
}

#[test]
fn test_preset_include_and_inheritance_resolve_relative_to_the_including_file() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path().join("presets/nested")).expect("preset directory");
    fs::write(
        fixture.path().join("presets/nested/base.json"),
        json!({"version":6,"configurePresets":[{"name":"base","hidden":true,"binaryDir":"${sourceDir}/build/infra/${presetName}","cacheVariables":{"CMAKE_TOOLCHAIN_FILE":{"type":"FILEPATH","value":"conan/conan_toolchain.cmake"},"PARENT_ONLY":"kept"}}]}).to_string(),
    ).expect("base preset");
    fs::write(
        fixture.path().join("presets/parents.json"),
        json!({"version":6,"include":["nested/base.json"],"configurePresets":[{"name":"parent","inherits":["base"],"cacheVariables":{"CMAKE_BUILD_TYPE":"Debug"}}]}).to_string(),
    ).expect("parent preset");
    write_presets(
        &fixture,
        &json!({"version":6,"include":["presets/parents.json"],"configurePresets":[{"name":"infra-debug","inherits":"parent"}]}),
    );
    assert_code(&fixture.run(&["prepare", "--context", "linux-debug"]), 0);
    assert_eq!(
        fixture.report()["data"]["dependency_context"]["binary_dir"],
        json!(fixture.path().join("build/infra/infra-debug"))
    );
}
