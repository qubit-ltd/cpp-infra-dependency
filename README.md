# cpp-infra-dependency

[![CI](https://github.com/qubit-ltd/cpp-infra-dependency/actions/workflows/ci.yml/badge.svg)](https://github.com/qubit-ltd/cpp-infra-dependency/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/qubit-cpp-infra-dependency.svg)](https://crates.io/crates/qubit-cpp-infra-dependency)
[![Documentation](https://docs.rs/qubit-cpp-infra-dependency/badge.svg)](https://docs.rs/qubit-cpp-infra-dependency)
[![Rust](https://img.shields.io/badge/rust-1.94%2B-blue.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![简体中文](https://img.shields.io/badge/语言-简体中文-blue.svg)](README.zh_CN.md)

Enforce a shared Conan 2 dependency baseline before CMake builds. C++ maintainers can inventory host and build requirements, reject unapproved direct dependencies, validate resolved transitive versions, and export checked CMake dependency inputs. Conan evaluates recipes and owns dependency resolution; this tool checks the resulting JSON graph and the project's committed lockfiles.

## Installation

Build the unpublished development source with Rust 1.94 or newer:

```sh
cargo install --locked --path .
cpp-infra-dependency --version --json
```

Provide Conan 2.28–2.33 on `PATH`. The adapter is exercised with real Conan 2.33.0 and a 2.28 fixture; intermediate versions are accepted within that JSON contract, without a claim of individual runtime certification. Conan profiles must be committed project files, with project-contained literal include chains using explicit `./`, `../` or absolute project paths. Bare include names are rejected because Conan may resolve them from its home. Input symlinks and inputs under digest-excluded build/cache/transaction directories are rejected. Dynamic Jinja profiles and Conan-home profile includes are rejected. No profile detection or compiler installation runs automatically.

## Quick start

A library must build with the same dependency choices locally and in CI. Start with the project-owned configurations in [conf/dependency/config.toml](conf/dependency/config.toml) and [the example profile](conf/dependency/profiles/linux-gcc), installed under `.infra/dependency/`. Register approved direct require strings and optional transitive version constraints in [the baseline](conf/dependency/policy/baseline.toml). The shipped baseline approves no external dependency versions.

The project's `.infra/project.toml` binds `profiles.debug.conan_context = "linux-debug"`; its `configure` preset is `infra-debug`. `CMakePresets.json` schema 6 must place that preset's `binaryDir` under `build/infra/` and explicitly point its `toolchainFile` at `build/infra/infra-debug/conan/conan_toolchain.cmake`. A minimal project-owned recipe is:

```ini
[generators]
CMakeDeps
CMakeToolchain
```

Run the complete first-install path against that project:

```sh
cpp-infra-dependency --project /path/to/library plan --context linux-debug
cpp-infra-dependency --project /path/to/library --offline lock update --context linux-debug
cpp-infra-dependency --project /path/to/library --offline inventory --context linux-debug
cpp-infra-dependency --project /path/to/library --offline check --context linux-debug
cpp-infra-dependency --project /path/to/library --offline prepare \
  --context linux-debug --output build/dependency-context.json
```

Inspect `build/dependency-context.json`: it identifies the profile, baseline, source/config/lock/graph digests, generated-file hashes, Conan settings and fingerprint. Then run the project's CMake configure/build presets, or let `cpp-infra-verify` consume that context. When packages are required, populate the local Conan cache first or explicitly register remote names and URLs in baseline `[remotes]` and select those names in the context. No remote is enabled implicitly. Missing cache entries fail in offline mode.

## Commands and safeguards

| Command | Result |
| --- | --- |
| `plan --context NAME` | JSON command plan; starts no external process. |
| `inventory --context NAME` | JSON nodes/edges, original direct requirements, host/build/test categories and full identity. |
| `check --context NAME` | Locked resolution and direct/resolved baseline gates; preserves recipe and lock. |
| `prepare --context NAME [--output FILE]` | Checks policy, installs with `--build=never` by default, verifies the actual generators folder, writes a dependency context. |
| `sync [--dry-run]` | Updates simple `[requires]`/`[tool_requires]` text entries; preserves comments and unrelated sections; validates all contexts in a staging project. |
| `lock update --context NAME [--dry-run]` | Creates a candidate lock, shows added/removed references and revisions, validates policy, atomically replaces only that context's lock. |

`sync` refuses Python editing and ambiguous text syntax, with canonical requirement suggestions. Python recipes remain fully supported for inventory, check, lock update and prepare. Synchronization does not refresh locks: follow it with `lock update` and the project's verification. Failed validation leaves the original recipe or lock in place. Dry runs retain reports/candidates, without replacing project inputs.

Every enabled context needs its own lock for normal checks. Direct policy covers external `requires`, `tool_requires` and `test_requires`; `internal_patterns` only exempts the direct-name rule. Resolved rules apply to every matching node in every context. Version constraints accept numeric `major.minor.patch`, `=`, `==`, `<`, `<=`, `>`, `>=` and comma intersections. Other version forms require an exact `allow_refs` entry. Inventory retains user/channel, recipe revision, package ID and package revision independently.

Dependency context `--output` must be a JSON artifact under the project's `build/` or `target/`, outside generators and declared input roots. Existing files can be replaced only when they are recognized dependency contexts for the same project/context. Output safety and canonical identity are checked before installation and again before persistence.

`prepare` uses the same `binary_dir/.cpp-infra.lock` and `.cpp-infra-project.json` ownership marker as verify. It refuses unmarked nonempty directories, checks CMake's recorded source root, verifies source/config identity before and after installation, and requires CMakeDeps/CMakeToolchain. Conan layout that shifts the generators folder fails with the actual and expected paths; adjust the recipe's `folders.generators` to match the declared output. The profile's actual compiler/runtime/cppstd/build_type settings are exported for verify to compare against configured CMake values.

All commands accept `--project`, `--report`, `--offline` and `--run-context`. Successful reports contain the common schema-1 identity, commands, redacted logs in a unique invocation directory, diagnostics and SHA-256 artifacts. Source/config changes invalidate run contexts. Supplied runtime snapshots must match their digest, bundle, executable hashes, version/revision and protocol. A standalone source build reports `bundle_id="development"` and a null revision unless `CPP_INFRA_BUILD_REVISION` was supplied at compile time. Exit codes are 0 success, 1 gate/Conan failure, 2 configuration/protocol, 3 I/O/spawn, and 130 cancellation.

Root-level JSON reports are tracked in optional `source_excluded_paths` metadata shared by run contexts and dependency contexts. Only explicitly declared, normalized report files are excluded from source hashing; configuration hashing is unchanged. Existing business files and project inputs cannot be excluded or replaced. The tool validates outputs again immediately before writing a report.

Each command and lock wait has a configurable `timeout_seconds` budget (default 300). Unix cancellation kills the external process group. `--offline` passes Conan `--no-remote`; selected Python recipes are executed and are not network sandboxed. Conan's own cache may be populated by checks. vcpkg and arbitrary CMake FetchContent parsing are outside this tool's scope.

## Testing

```sh
cargo +nightly fmt --all -- --config-path .infra/style/rustfmt.toml --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

The default suite uses deterministic CLI fixtures. Three explicitly ignored acceptance tests require a real Conan executable and create isolated temporary Conan homes and local packages; run both without network or private services:

```sh
CONAN_TEST_EXECUTABLE=/absolute/path/to/conan \
  cargo test --locked --test conan_real_tests -- --ignored
```

## License

Licensed under the [Apache License 2.0](LICENSE).

## Contributing

Keep changes focused. Include regression tests for observable behavior, run the checks above, and update both README languages when changing the user contract. CI validates this crate itself.

## Author

Maintained by the [Qubit organization](https://github.com/qubit-ltd).
