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

fn assert_diagnostic(fixture: &Fixture, expected: &str) {
    let report = fixture.report();
    assert!(
        report["diagnostics"].to_string().contains(expected),
        "expected diagnostic {expected:?}: {}",
        report["diagnostics"]
    );
}

fn write_lock(fixture: &Fixture, lock: &Value) {
    fs::write(fixture.path().join("conan/debug.lock"), lock.to_string()).expect("lock fixture");
}

fn set_resolved_rule(fixture: &Fixture, rule: &str) {
    let path = fixture.path().join(".infra/dependency/policy/baseline.toml");
    let policy = fs::read_to_string(&path).expect("baseline fixture");
    fs::write(path, policy.replace(">=11.0.0,<12.0.0", rule)).expect("resolved policy fixture");
}

fn set_reference(fixture: &Fixture, reference: &str, requirement: &str) {
    let mut graph = fixture.graph();
    graph["graph"]["nodes"]["1"]["ref"] = json!(reference);
    graph["graph"]["nodes"]["0"]["dependencies"]["1"]["ref"] = json!(reference);
    graph["graph"]["nodes"]["0"]["dependencies"]["1"]["require"] = json!(requirement);
    fixture.put_graph(&graph);
}

fn set_candidate(fixture: &Fixture, candidate: &Value) {
    fs::write(fixture.path().join("fixture-candidate.json"), candidate.to_string()).expect("candidate fixture");
    let adapter = fixture.path().join("bin/conan");
    let script = fs::read_to_string(&adapter).expect("Conan adapter fixture");
    let original = "pathlib.Path(dest).write_text(json.dumps({'version':'0.5','requires':['fmt/11.0.2#abc%123'],'build_requires':[],'python_requires':[],'config_requires':[]}))";
    assert!(script.contains(original), "candidate write fixture changed");
    fs::write(
        adapter,
        script.replace(
            original,
            "pathlib.Path(dest).write_bytes((p/'fixture-candidate.json').read_bytes())",
        ),
    )
    .expect("candidate Conan adapter");
}

#[test]
fn test_graph_schema_errors_identify_the_invalid_protocol_field() {
    let cases = [
        ("/graph", None, "missing graph object"),
        ("/graph/nodes", None, "missing nodes"),
        ("/graph/nodes/0", None, "no root node 0"),
        ("/graph/nodes/1", Some(json!([])), "node 1: missing ref/package_id"),
        ("/graph/nodes/1/ref", None, "node 1: missing ref/package_id"),
        ("/graph/nodes/1/ref", Some(json!(17)), "node 1: missing ref/package_id"),
        (
            "/graph/nodes/1/ref",
            Some(Value::Null),
            "dependency node 1 missing reference",
        ),
        ("/graph/nodes/0/context", None, "unsupported context on Conan node 0"),
        (
            "/graph/nodes/1/context",
            Some(json!("target")),
            "unsupported context on Conan node 1",
        ),
        (
            "/graph/nodes/0/dependencies",
            None,
            "missing dependency edges on node 0",
        ),
        (
            "/graph/nodes/1/dependencies",
            Some(json!([])),
            "missing dependency edges on node 1",
        ),
        ("/graph/nodes/0/dependencies/1/ref", None, "dependency edge 0->1"),
        ("/graph/nodes/0/dependencies/1/require", None, "dependency edge 0->1"),
        (
            "/graph/nodes/0/dependencies/1/direct",
            Some(json!("true")),
            "dependency edge 0->1",
        ),
        ("/graph/nodes/0/generators", None, "missing root generators"),
        (
            "/graph/nodes/0/generators",
            Some(json!(["CMakeDeps", 7])),
            "invalid generator entry",
        ),
        ("/graph/nodes/0/settings", None, "missing Conan root settings"),
        (
            "/graph/nodes/0/settings",
            Some(json!([])),
            "missing Conan root settings",
        ),
    ];
    for (pointer, replacement, expected) in cases {
        let fixture = Fixture::new();
        let mut graph = fixture.graph();
        if let Some(value) = replacement {
            *graph.pointer_mut(pointer).expect("existing graph field") = value;
        } else {
            let (parent, field) = pointer.rsplit_once('/').expect("graph pointer");
            graph
                .pointer_mut(parent)
                .and_then(Value::as_object_mut)
                .expect("graph field parent")
                .remove(field);
        }
        fixture.put_graph(&graph);
        assert_code(&fixture.run(&["inventory", "--context", "linux-debug"]), 2);
        assert_diagnostic(&fixture, expected);
        assert!(
            fixture.report()["data"]["inventory"].is_null(),
            "invalid graph was inventoried: {pointer}"
        );
    }
}

#[test]
fn test_graph_rejects_dangling_edges_and_reports_resolution_errors() {
    let fixture = Fixture::new();
    let mut graph = fixture.graph();
    graph["graph"]["nodes"]["0"]["dependencies"]["99"] =
        json!({"ref":"missing/1.0.0#r","require":"missing/1.0.0","direct":false});
    fixture.put_graph(&graph);
    assert_code(&fixture.run(&["check", "--context", "linux-debug"]), 2);
    assert_diagnostic(&fixture, "unsupported Conan dependency edge 0->99");

    let fixture = Fixture::new();
    let mut graph = fixture.graph();
    graph["graph"]["error"] = json!({"type":"MissingRecipe","ref":"fmt/11.0.2"});
    fixture.put_graph(&graph);
    assert_code(&fixture.run(&["inventory", "--context", "linux-debug"]), 1);
    assert_diagnostic(&fixture, "Conan graph resolution error");
    assert_diagnostic(&fixture, "MissingRecipe");
}

#[test]
fn test_graph_reference_errors_are_configuration_failures() {
    for (reference, expected) in [
        ("fmt", "invalid Conan reference: fmt"),
        ("/11.0.2#abc", "invalid Conan reference: /11.0.2#abc"),
        ("fmt/#abc", "invalid Conan reference: fmt/#abc"),
        ("fmt/11.0.2@owner#abc", "invalid user/channel: fmt/11.0.2@owner#abc"),
    ] {
        let fixture = Fixture::new();
        set_reference(&fixture, reference, "fmt/11.0.2");
        assert_code(&fixture.run(&["inventory", "--context", "linux-debug"]), 2);
        assert_diagnostic(&fixture, expected);
    }

    let fixture = Fixture::new();
    set_reference(&fixture, "fmt/11.0.2#abc", "fmt");
    assert_code(&fixture.run(&["check", "--context", "linux-debug"]), 2);
    assert_diagnostic(&fixture, "invalid Conan reference: fmt");
}

#[test]
fn test_inventory_preserves_user_channel_revisions_and_optional_node_fields() {
    let fixture = Fixture::new();
    set_reference(&fixture, "fmt/11.0.2@owner/stable#abc", "fmt/11.0.2@owner/stable");
    let mut graph = fixture.graph();
    graph["graph"]["nodes"]["1"]["rrev"] = json!("ignored-fallback");
    graph["graph"]["nodes"]["1"]["prev"] = json!("package-revision");
    graph["graph"]["nodes"]["1"]["settings"] = json!({"arch":"x86_64"});
    graph["graph"]["nodes"]["1"]["options"] = json!({"shared":"False"});
    fixture.put_graph(&graph);
    let policy_path = fixture.path().join(".infra/dependency/policy/baseline.toml");
    fs::write(
        &policy_path,
        fs::read_to_string(&policy_path)
            .expect("policy")
            .replace("fmt/11.0.2'", "fmt/11.0.2@owner/stable'"),
    )
    .expect("user/channel direct policy");
    write_lock(
        &fixture,
        &json!({"version":"0.5","requires":["fmt/11.0.2@owner/stable#abc%123"],"build_requires":[],"python_requires":[]}),
    );
    assert_code(&fixture.run(&["inventory", "--context", "linux-debug"]), 0);
    let report = fixture.report();
    let inventory = &report["data"]["inventory"];
    let node = &inventory["nodes"][0];
    assert_eq!(node["user"], "owner");
    assert_eq!(node["channel"], "stable");
    assert_eq!(node["recipe_revision"], "abc");
    assert_eq!(node["package_id"], "package1");
    assert_eq!(node["package_revision"], "package-revision");
    assert_eq!(node["binary"], "Cache");
    assert_eq!(node["settings"], json!({"arch":"x86_64"}));
    assert_eq!(node["options"], json!({"shared":"False"}));
    assert!(node["python_requires"].is_null());
    assert_eq!(inventory["direct"][0]["requirement"], "fmt/11.0.2@owner/stable");
}

#[test]
fn test_inventory_uses_rrev_fallback_and_classifies_direct_test_and_tool_requirements() {
    let fixture = Fixture::new();
    let mut graph = fixture.graph();
    graph["graph"]["nodes"]["1"]["ref"] = json!("fmt/11.0.2");
    graph["graph"]["nodes"]["1"]["rrev"] = json!("fallback");
    graph["graph"]["nodes"]["0"]["dependencies"]["1"]["ref"] = json!("fmt/11.0.2");
    graph["graph"]["nodes"]["0"]["dependencies"]["1"]["test"] = json!(true);
    graph["graph"]["nodes"]["0"]["dependencies"]["2"] =
        json!({"ref":"qubit-tool/1.0.0#r","require":"qubit-tool/1.0.0","direct":true,"build":true});
    graph["graph"]["nodes"]["2"] =
        json!({"ref":"qubit-tool/1.0.0#r","context":"build","package_id":null,"dependencies":{}});
    graph["graph"]["nodes"]["0"]["dependencies"]["3"] =
        json!({"ref":"qubit-support/1.0.0#r","require":"qubit-support/1.0.0","direct":false});
    graph["graph"]["nodes"]["3"] =
        json!({"ref":"qubit-support/1.0.0#r","context":"host","package_id":null,"dependencies":{}});
    fixture.put_graph(&graph);
    // A raw graph reference may omit the revision while Conan exposes rrev
    // separately.
    write_lock(
        &fixture,
        &json!({"version":"0.5","requires":["fmt/11.0.2","qubit-support/1.0.0#r"],"build_requires":["qubit-tool/1.0.0#r"],"python_requires":[]}),
    );
    // Inventory parsing is observable even when the subsequent lock gate rejects an
    // unpinned entry.
    assert_code(&fixture.run(&["inventory", "--context", "linux-debug"]), 2);
    assert_diagnostic(&fixture, "must pin a recipe revision");
    let report = fixture.report();
    let inventory = &report["data"]["inventory"];
    assert_eq!(inventory["nodes"][0]["recipe_revision"], "fallback");
    assert!(inventory["nodes"][0]["user"].is_null());
    assert!(inventory["nodes"][0]["channel"].is_null());
    assert!(inventory["nodes"][0]["settings"].is_null());
    assert_eq!(inventory["direct"].as_array().expect("direct inventory").len(), 2);
    assert_eq!(inventory["direct"][0]["category"], "test_requires");
    assert_eq!(inventory["direct"][1]["category"], "tool_requires");
    assert_eq!(inventory["direct"][1]["context"], "build");
    assert_eq!(inventory["edges"]["0"]["3"]["direct"], false);
    assert!(inventory["nodes"][1]["package_id"].is_null());
    assert!(inventory["nodes"][1]["binary"].is_null());
}

#[test]
fn test_resolved_comparisons_apply_inclusive_exclusive_and_equality_boundaries() {
    for (rule, code) in [
        (">=11.0.2", 0),
        (">=11.0.3", 1),
        ("<=11.0.2", 0),
        ("<=11.0.1", 1),
        (">11.0.2", 1),
        (">11.0.1", 0),
        ("<11.0.2", 1),
        ("<11.0.3", 0),
        ("=11.0.2", 0),
        ("=11.0.3", 1),
        ("==11.0.2", 0),
        ("==11.0.1", 1),
        ("11.0.2", 0),
        ("11.0.1", 1),
        (" >= 11.0.2 , < 12.0.0 ", 0),
        (">10.99.99,<11.1.0", 0),
        (">=11.0.3,<=11.0.2", 1),
    ] {
        let fixture = Fixture::new();
        set_resolved_rule(&fixture, rule);
        assert_code(&fixture.run(&["check", "--context", "linux-debug"]), code);
        if code == 0 {
            assert_eq!(fixture.report()["status"], "passed", "range {rule}");
        } else {
            assert_diagnostic(&fixture, &format!("resolved fmt/11.0.2#abc in host violates {rule}"));
        }
    }
}

#[test]
fn test_resolved_policy_rejects_unsupported_range_syntax_before_running_conan() {
    for rule in [
        "",
        "^11.0.2",
        "~11.0.2",
        "!=11.0.2",
        "11.*",
        ">=11.0",
        "11.00.2",
        "11..2",
        "11.0.rc1",
        "11.0.2,",
        "18446744073709551616.0.0",
    ] {
        let fixture = Fixture::new();
        set_resolved_rule(&fixture, rule);
        assert_code(&fixture.run(&["check", "--context", "linux-debug"]), 2);
        assert_diagnostic(&fixture, "resolved range accepts only numeric three-part comparisons");
        assert!(
            fixture.report()["commands"]
                .as_array()
                .expect("command list")
                .is_empty(),
            "invalid range reached Conan: {rule}"
        );
    }
}

#[test]
fn test_unsupported_resolved_versions_need_an_exact_unrevisioned_allow_reference() {
    for version in [
        "11.0",
        "11.0.2.1",
        "11.00.2",
        "11..2",
        "11.0.rc1",
        "18446744073709551616.0.0",
    ] {
        let fixture = Fixture::new();
        let requirement = format!("fmt/{version}");
        let reference = format!("{requirement}#abc");
        set_reference(&fixture, &reference, &requirement);
        let policy = fixture.path().join(".infra/dependency/policy/baseline.toml");
        let text = fs::read_to_string(&policy)
            .expect("policy")
            .replace("fmt/11.0.2", &requirement);
        fs::write(&policy, &text).expect("matching direct requirement");
        write_lock(
            &fixture,
            &json!({"version":"0.5","requires":[reference],"build_requires":[],"python_requires":[]}),
        );
        assert_code(&fixture.run(&["check", "--context", "linux-debug"]), 1);
        assert_diagnostic(&fixture, "uses unsupported version; register exact allow_refs");
        fs::write(
            &policy,
            text.replace("allow_refs = []", &format!("allow_refs = ['{requirement}']")),
        )
        .expect("exact unrevisioned allowance");
        assert_code(&fixture.run(&["check", "--context", "linux-debug"]), 0);
    }
}

#[test]
fn test_exact_allow_refs_bypass_resolved_ranges_but_keep_direct_identity() {
    for (allowance, code) in [
        ("fmt/11.0.2#abc", 0),
        ("fmt/11.0.2", 0),
        ("fmt/11.0.2#different", 1),
        ("fmt/11.0.2@owner/stable", 1),
        ("fmt/11.0.20", 1),
    ] {
        let fixture = Fixture::new();
        set_resolved_rule(&fixture, ">12.0.0");
        let policy = fixture.path().join(".infra/dependency/policy/baseline.toml");
        fs::write(
            &policy,
            fs::read_to_string(&policy)
                .expect("policy")
                .replace("allow_refs = []", &format!("allow_refs = ['{allowance}']")),
        )
        .expect("exact reference allowance");
        assert_code(&fixture.run(&["check", "--context", "linux-debug"]), code);
        if code == 0 {
            assert_eq!(fixture.report()["status"], "passed");
        } else {
            assert_diagnostic(&fixture, "resolved fmt/11.0.2#abc in host violates >12.0.0");
        }
    }

    let fixture = Fixture::new();
    set_reference(&fixture, "fmt/11.0.2#abc", "fmt/[>=11.0.0 <12.0.0]");
    let policy = fixture.path().join(".infra/dependency/policy/baseline.toml");
    fs::write(
        &policy,
        fs::read_to_string(&policy)
            .expect("policy")
            .replace("allow_refs = []", "allow_refs = ['fmt/11.0.2#abc']"),
    )
    .expect("allow resolved reference");
    assert_code(&fixture.run(&["check", "--context", "linux-debug"]), 1);
    assert_diagnostic(&fixture, "direct fmt must use fmt/11.0.2, got fmt/[>=11.0.0 <12.0.0]");
}

#[test]
fn test_internal_patterns_match_exact_names_and_nonoverlapping_wildcard_parts() {
    for (pattern, code) in [
        ("fmt", 0),
        ("f*t", 0),
        ("fm*t", 0),
        ("fmt*", 0),
        ("*fmt", 0),
        ("fm*mt", 1),
        ("Fmt", 1),
    ] {
        let fixture = Fixture::new();
        let policy = fixture.path().join(".infra/dependency/policy/baseline.toml");
        fs::write(
            &policy,
            fs::read_to_string(&policy)
                .expect("policy")
                .replace(
                    "internal_patterns = ['qubit-*']",
                    &format!("internal_patterns = ['{pattern}']"),
                )
                .replace("fmt = 'fmt/11.0.2'\n", ""),
        )
        .expect("internal direct policy");
        assert_code(&fixture.run(&["check", "--context", "linux-debug"]), code);
        if code == 0 {
            assert_eq!(fixture.report()["status"], "passed");
        } else {
            assert_diagnostic(&fixture, "unregistered external direct requires fmt/11.0.2 (host)");
        }
    }
}

#[test]
fn test_baseline_rejects_invalid_reference_identity_and_internal_patterns() {
    for (from, to, expected) in [
        ("schema = 1", "schema = 2", "baseline requires schema=1 and baseline_id"),
        (
            "baseline_id = 'test-1'",
            "baseline_id = ''",
            "baseline requires schema=1 and baseline_id",
        ),
        (
            "fmt = 'fmt/11.0.2'",
            "fmt = 'zlib/11.0.2'",
            "baseline direct key differs from require name: fmt",
        ),
        ("fmt = 'fmt/11.0.2'", "fmt = 'fmt'", "invalid Conan reference: fmt"),
        (
            "allow_refs = []",
            "allow_refs = ['fmt/1.0.0@owner']",
            "invalid user/channel",
        ),
        (
            "internal_patterns = ['qubit-*']",
            "internal_patterns = ['']",
            "unsupported internal name pattern",
        ),
        (
            "internal_patterns = ['qubit-*']",
            "internal_patterns = ['qubit-**']",
            "unsupported internal name pattern",
        ),
        (
            "internal_patterns = ['qubit-*']",
            "internal_patterns = ['qubit-?']",
            "unsupported internal name pattern",
        ),
        (
            "internal_patterns = ['qubit-*']",
            "internal_patterns = ['qubit-[ab]']",
            "unsupported internal name pattern",
        ),
    ] {
        let fixture = Fixture::new();
        let policy = fixture.path().join(".infra/dependency/policy/baseline.toml");
        fs::write(&policy, fs::read_to_string(&policy).expect("policy").replace(from, to)).expect("invalid baseline");
        assert_code(&fixture.run(&["check", "--context", "linux-debug"]), 2);
        assert_diagnostic(&fixture, expected);
        assert!(
            fixture.report()["commands"]
                .as_array()
                .expect("command list")
                .is_empty()
        );
    }
}

#[test]
fn test_lock_schema_and_revision_errors_are_configuration_failures() {
    let valid = json!({"version":"0.5","requires":["fmt/11.0.2#abc%123"],"build_requires":[],"python_requires":[]});
    for (pointer, replacement, expected) in [
        ("/version", json!("0.4"), "expected version 0.5"),
        ("/version", json!(0.5), "expected version 0.5"),
        ("/version", Value::Null, "expected version 0.5"),
        ("/requires", Value::Null, "Conan lock missing requires"),
        ("/build_requires", json!({}), "Conan lock missing build_requires"),
        ("/python_requires", Value::Null, "Conan lock missing python_requires"),
        (
            "/requires",
            json!(["fmt/11.0.2"]),
            "Conan lock requires entry must pin a recipe revision",
        ),
        (
            "/build_requires",
            json!([42]),
            "Conan lock build_requires entry must pin a recipe revision",
        ),
        (
            "/python_requires",
            json!(["pytool/1.0.0"]),
            "Conan lock python_requires entry must pin a recipe revision",
        ),
    ] {
        let fixture = Fixture::new();
        let mut lock = valid.clone();
        *lock.pointer_mut(pointer).expect("lock field") = replacement;
        write_lock(&fixture, &lock);
        assert_code(&fixture.run(&["check", "--context", "linux-debug"]), 2);
        assert_diagnostic(&fixture, expected);
    }
    let fixture = Fixture::new();
    fs::write(fixture.path().join("conan/debug.lock"), "{broken lock").expect("malformed lock");
    assert_code(&fixture.run(&["check", "--context", "linux-debug"]), 2);
    assert_diagnostic(&fixture, "invalid Conan lock");
    assert_diagnostic(&fixture, "debug.lock");
}

#[test]
fn test_lock_covers_each_context_and_python_revision_ignoring_timestamps() {
    let fixture = Fixture::new();
    let mut graph = fixture.graph();
    graph["graph"]["nodes"]["1"]["python_requires"] = json!({"python-base":{"ref":"python-base/1.0.0#pyrev"}});
    graph["graph"]["nodes"]["1"]["dependencies"]["2"] =
        json!({"ref":"qubit-tool/1.0.0#toolrev","require":"qubit-tool/1.0.0","direct":false,"build":true});
    graph["graph"]["nodes"]["2"] =
        json!({"ref":"qubit-tool/1.0.0#toolrev","context":"build","package_id":"tool","dependencies":{}});
    fixture.put_graph(&graph);
    for (requires, build_requires, python_requires, code, diagnostic) in [
        (
            json!(["fmt/11.0.2#abc%123"]),
            json!([]),
            json!(["python-base/1.0.0#pyrev%9"]),
            1,
            "lock does not cover qubit-tool/1.0.0#toolrev in build",
        ),
        (
            json!(["fmt/11.0.2#abc%123"]),
            json!(["qubit-tool/1.0.0#toolrev%8"]),
            json!([]),
            1,
            "lock does not cover Python require python-base/1.0.0#pyrev",
        ),
        (
            json!(["fmt/11.0.2#abc%123"]),
            json!(["qubit-tool/1.0.0#toolrev%8"]),
            json!(["python-base/1.0.0#old%9"]),
            1,
            "lock does not cover Python require python-base/1.0.0#pyrev",
        ),
        (
            json!(["fmt/11.0.2#old%123"]),
            json!(["qubit-tool/1.0.0#toolrev%8"]),
            json!(["python-base/1.0.0#pyrev%9"]),
            1,
            "lock does not cover fmt/11.0.2#abc in host",
        ),
        (
            json!(["fmt/11.0.2#abc%123"]),
            json!(["qubit-tool/1.0.0#toolrev%8"]),
            json!(["python-base/1.0.0#pyrev%9"]),
            0,
            "",
        ),
    ] {
        write_lock(
            &fixture,
            &json!({"version":"0.5","requires":requires,"build_requires":build_requires,"python_requires":python_requires}),
        );
        assert_code(&fixture.run(&["check", "--context", "linux-debug"]), code);
        if code == 0 {
            assert_eq!(fixture.report()["status"], "passed");
        } else {
            assert_diagnostic(&fixture, diagnostic);
        }
    }
}

#[test]
fn test_lock_update_creates_a_missing_lock_and_reports_the_candidate() {
    let fixture = Fixture::new();
    let config = fixture.path().join(".infra/dependency/config.toml");
    fs::write(
        &config,
        fs::read_to_string(&config)
            .expect("config")
            .replace("conan/debug.lock", "conan/new.lock"),
    )
    .expect("new lock path");
    let original_lock = fs::read(fixture.path().join("conan/debug.lock")).expect("other lock");
    assert_code(&fixture.run(&["lock", "update", "--context", "linux-debug"]), 0);
    let lock: Value = serde_json::from_slice(&fs::read(fixture.path().join("conan/new.lock")).expect("created lock"))
        .expect("created lock JSON");
    assert_eq!(lock["requires"], json!(["fmt/11.0.2#abc%123"]));
    assert_eq!(
        fs::read(fixture.path().join("conan/debug.lock")).expect("other lock preserved"),
        original_lock
    );
    let report = fixture.report();
    let update = &report["data"]["lock_update"];
    assert_eq!(update["context"], "linux-debug");
    assert_eq!(update["dry_run"], false);
    assert_eq!(update["requires_verification"], true);
    assert_eq!(update["added"], json!(["requires:fmt/11.0.2#abc%123"]));
    assert_eq!(update["removed"], json!([]));
    assert_diagnostic(&fixture, "verification-required");
    let candidate = report["artifacts"]
        .as_array()
        .expect("artifacts")
        .iter()
        .find(|artifact| artifact["kind"] == "dependency-lock-candidate")
        .expect("candidate artifact");
    let root = std::path::PathBuf::from(report["report_root"].as_str().expect("report root"));
    assert_eq!(
        fs::read(root.join(candidate["path"].as_str().expect("candidate path"))).expect("candidate bytes"),
        fs::read(fixture.path().join("conan/new.lock")).expect("new lock bytes")
    );
}

#[test]
fn test_lock_update_diff_includes_config_references_and_preserves_dry_run_bytes() {
    let fixture = Fixture::new();
    write_lock(
        &fixture,
        &json!({"version":"0.5","requires":["fmt/10.0.0#old"],"build_requires":[],"python_requires":[],"config_requires":["config/1.0.0#old"]}),
    );
    set_candidate(
        &fixture,
        &json!({"version":"0.5","requires":["fmt/11.0.2#abc%123"],"build_requires":[],"python_requires":[],"config_requires":["config/2.0.0#new","config/2.0.0#new"]}),
    );
    let path = fixture.path().join("conan/debug.lock");
    let old = fs::read(&path).expect("old lock");
    assert_code(
        &fixture.run(&["lock", "update", "--context", "linux-debug", "--dry-run"]),
        0,
    );
    assert_eq!(fs::read(&path).expect("dry-run lock"), old);
    let report = fixture.report();
    assert_eq!(report["data"]["lock_update"]["dry_run"], true);
    assert_eq!(
        report["data"]["lock_update"]["added"],
        json!(["config_requires:config/2.0.0#new", "requires:fmt/11.0.2#abc%123"])
    );
    assert_eq!(
        report["data"]["lock_update"]["removed"],
        json!(["config_requires:config/1.0.0#old", "requires:fmt/10.0.0#old"])
    );
    assert_code(&fixture.run(&["lock", "update", "--context", "linux-debug"]), 0);
    let installed: Value =
        serde_json::from_slice(&fs::read(&path).expect("installed lock")).expect("installed lock JSON");
    assert_eq!(installed["requires"], json!(["fmt/11.0.2#abc%123"]));
    assert_eq!(
        installed["config_requires"],
        json!(["config/2.0.0#new", "config/2.0.0#new"])
    );
}

#[test]
fn test_lock_update_preserves_old_bytes_when_old_lock_or_candidate_is_invalid() {
    for (old, candidate, expected) in [
        (
            "not JSON".to_owned(),
            json!({"version":"0.5","requires":["fmt/11.0.2#abc"],"build_requires":[],"python_requires":[]}),
            "invalid lock JSON",
        ),
        (
            json!({"version":"0.5","requires":[7]}).to_string(),
            json!({"version":"0.5","requires":["fmt/11.0.2#abc"],"build_requires":[],"python_requires":[]}),
            "non-string lock reference",
        ),
        (
            json!({"version":"0.5","requires":["fmt/11.0.2#abc"]}).to_string(),
            json!({"version":"0.5","requires":["fmt/11.0.2#abc"],"build_requires":[],"python_requires":[],"config_requires":[false]}),
            "non-string lock reference",
        ),
        (
            json!({"version":"0.5","requires":["fmt/11.0.2#abc"]}).to_string(),
            json!({"version":"0.4","requires":["fmt/11.0.2#abc"],"build_requires":[],"python_requires":[]}),
            "unsupported Conan lock schema",
        ),
    ] {
        let fixture = Fixture::new();
        let path = fixture.path().join("conan/debug.lock");
        fs::write(&path, &old).expect("old lock fixture");
        set_candidate(&fixture, &candidate);
        assert_code(&fixture.run(&["lock", "update", "--context", "linux-debug"]), 2);
        assert_diagnostic(&fixture, expected);
        assert_eq!(fs::read_to_string(&path).expect("preserved old lock"), old);
        assert!(fixture.report()["data"]["lock_update"].is_null());
    }
}

#[test]
fn test_lock_update_preserves_the_old_lock_when_recipe_changes_project_inputs() {
    let fixture = Fixture::new();
    let old = fs::read(fixture.path().join("conan/debug.lock")).expect("old lock");
    let adapter = fixture.path().join("bin/conan");
    let script = fs::read_to_string(&adapter).expect("adapter").replace(
        "if a[:2] == ['lock','create']:",
        "if a[:2] == ['lock','create']:\n (p/'src/changed.cpp').write_text('int changed;')",
    );
    fs::write(adapter, script).expect("input-mutating Conan adapter");
    assert_code(&fixture.run(&["lock", "update", "--context", "linux-debug"]), 2);
    assert_diagnostic(
        &fixture,
        "project inputs changed during lock validation; old lock preserved",
    );
    assert_eq!(
        fs::read(fixture.path().join("conan/debug.lock")).expect("preserved lock"),
        old
    );
}
