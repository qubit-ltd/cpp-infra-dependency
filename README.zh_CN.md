# cpp-infra-dependency

[![CI](https://github.com/qubit-ltd/cpp-infra-dependency/actions/workflows/ci.yml/badge.svg)](https://github.com/qubit-ltd/cpp-infra-dependency/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/qubit-cpp-infra-dependency.svg)](https://crates.io/crates/qubit-cpp-infra-dependency)
[![Documentation](https://docs.rs/qubit-cpp-infra-dependency/badge.svg)](https://docs.rs/qubit-cpp-infra-dependency)
[![Rust](https://img.shields.io/badge/rust-1.94%2B-blue.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![English](https://img.shields.io/badge/language-English-blue.svg)](README.md)

在 CMake 构建前统一检查 Conan 2 依赖。维护 C++ 库时，可以用本工具盘点主机和构建依赖，阻止未经登记的直接依赖，校验传递依赖版本，并导出经过检查的构建输入。recipe 的执行和依赖解析由 Conan 完成，本工具读取真实 JSON 依赖图，结合项目提交的锁文件执行基线检查。

## 安装

当前是尚未发布的开发源码，需要 Rust 1.94 或更新版本：

```sh
cargo install --locked --path .
cpp-infra-dependency --version --json
```

请把 Conan 2.28–2.33 放入 `PATH`。适配器已经过 Conan 2.33.0 的真实验收，并有 2.28 格式 fixture；区间内版本按同一 JSON 契约接受，不代表每个版本都已单独实测。build/host profile 及其字面量 include 链必须是项目内可提交的普通文件；include 使用显式 `./`、`../` 或项目内绝对路径，避免同名文件被 Conan home 覆盖。拒绝输入符号链接，以及位于摘要排除的构建、缓存或事务目录内的输入；不接受动态 Jinja profile 或 Conan home 中的 profile 引用。工具不会自动探测 profile 或安装编译器。

## 快速开始

假设你维护一个 C++ 库，希望开发机和 CI 使用相同的依赖版本。先把 [配置示例](conf/dependency/config.toml)和 [profile 示例](conf/dependency/profiles/linux-gcc)放到项目的 `.infra/dependency/` 下，按实际环境调整编译器。然后在[组织基线](conf/dependency/policy/baseline.toml)中登记允许使用的直接 require 字符串，以及需要约束的传递依赖版本。交付基线没有预设任何已批准的外部依赖版本。

项目自己的 `.infra/project.toml` 通过 `profiles.debug.conan_context = "linux-debug"` 绑定依赖配置，`configure` 对应 `infra-debug`。`CMakePresets.json` 使用 schema 6，将该 preset 的 `binaryDir` 放在 `build/infra/` 内，并用 `toolchainFile` 明确指向 `build/infra/infra-debug/conan/conan_toolchain.cmake`。最小 recipe 如下：

```ini
[generators]
CMakeDeps
CMakeToolchain
```

在这个项目上执行首次接入流程：

```sh
cpp-infra-dependency --project /path/to/library plan --context linux-debug
cpp-infra-dependency --project /path/to/library --offline lock update --context linux-debug
cpp-infra-dependency --project /path/to/library --offline inventory --context linux-debug
cpp-infra-dependency --project /path/to/library --offline check --context linux-debug
cpp-infra-dependency --project /path/to/library --offline prepare \
  --context linux-debug --output build/dependency-context.json
```

检查 `build/dependency-context.json`，可看到 profile、基线标识、源码/配置/锁文件/依赖图摘要，以及生成文件哈希、Conan 设置和指纹。下一步可以执行项目的 CMake configure/build preset，也可以交给 `cpp-infra-verify` 消费该上下文。如果项目有包依赖，应先准备 Conan 本地缓存，或在基线 `[remotes]` 中登记名称和 URL，再由 context 显式选用；工具不会默认启用远端。离线模式缺少缓存时会失败。

## 命令与约束

| 命令 | 行为 |
| --- | --- |
| `plan --context NAME` | 输出 JSON 命令计划，不启动外部进程。 |
| `inventory --context NAME` | 输出节点、边、原始直接声明、主机/构建/测试类别及完整包身份。 |
| `check --context NAME` | 在锁文件约束下解析并检查基线，不替换 recipe 或锁文件。 |
| `prepare --context NAME [--output FILE]` | 检查策略后安装依赖，默认 `--build=never`；验证真实生成目录并写上下文。 |
| `sync [--dry-run]` | 只调整 txt 中简单的 `[requires]`/`[tool_requires]` 条目，保留注释和其他段落，在暂存项目中验证所有 context。 |
| `lock update --context NAME [--dry-run]` | 建立候选锁，展示引用及 revision 增删，检查策略后原子替换所选 context 的锁。 |

`sync` 遇到 Python recipe 或无法安全保留的文本结构会拒绝自动修改，并提示规范 require 字符串；Python recipe 可正常用于其余四类操作。同步依赖声明不会更新锁文件，应随后执行 `lock update`，再验证项目。暂存校验失败时保留原 recipe 或锁文件。`--dry-run` 会保留报告和候选结果，但不替换项目输入。

正常检查要求每个启用的 context 都有独立锁文件。外部 `requires`、`tool_requires` 和 `test_requires` 的直接声明均受基线约束，`internal_patterns` 只豁免直接依赖名称登记；同名节点无论处于哪个 context，都要逐一执行命中的 resolved 规则。版本规则仅支持数字三段版本、`=`、`==`、`<`、`<=`、`>`、`>=` 和逗号交集；其他命名必须加入精确 `allow_refs`。盘点分别保留 user/channel、recipe revision、package ID 和 package revision。

依赖上下文的 `--output` 必须是项目 `build/` 或 `target/` 下的 JSON 产物，并且不在生成器目录或声明的输入目录内。已有文件只有属于同一项目、同一 context 的依赖上下文时才允许替换。安装前和持久化前都会验证输出角色及规范路径身份。

`prepare` 与 verify 共用 `binary_dir/.cpp-infra.lock` 和 `.cpp-infra-project.json` 管理标记，不接管没有标记的非空目录，并检查 CMake 缓存记录的源码根目录。安装前后均复核源码和配置身份，同时要求生成 CMakeDeps/CMakeToolchain。recipe layout 导致实际生成目录偏移时，工具报告实际路径与声明路径并失败；应调整 recipe 的 `folders.generators`。上下文保存 Conan 实际 compiler/runtime/cppstd/build_type 设置，由 verify 对照 CMake 配置后的真实值。

所有命令接受 `--project`、`--report`、`--offline`、`--run-context`。成功报告遵守公共 schema 1，包含身份、命令、每次调用独占目录中的脱敏日志、诊断和 SHA-256 产物。源码或配置发生变化后，旧运行上下文失效；提供运行时快照时，还会检查快照摘要、bundle、可执行文件哈希、版本/revision 和协议。独立源码构建使用 `bundle_id="development"`，除非编译时提供 `CPP_INFRA_BUILD_REVISION`，否则 revision 为 null。退出码分别为：0 成功、1 策略或 Conan 失败、2 配置/协议错误、3 I/O 或启动失败、130 取消。

根目录 JSON 报告通过可选的 `source_excluded_paths` 元数据与运行上下文、依赖上下文共享。源码摘要只排除明确声明且路径规范的报告文件，配置摘要不受影响。业务文件和项目输入不能被排除或覆盖，最终写报告前还会再次检查目标。

单条命令和目录锁等待均受 `timeout_seconds` 限制，默认 300 秒；Unix 取消会终止外部进程组。`--offline` 向 Conan 传入 `--no-remote`，但不会给选中的 Python recipe 增加网络沙箱。普通检查可能填充 Conan 自己的缓存。本工具不解析 vcpkg 和任意 CMake FetchContent。

## 测试

```sh
cargo +nightly fmt --all -- --config-path .infra/style/rustfmt.toml --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

默认测试使用确定性的 CLI fixture。另有三项明确标为忽略的真实验收，需显式指定 Conan 可执行文件；测试使用临时 Conan home 和本地小包，不访问网络或私有服务：

```sh
CONAN_TEST_EXECUTABLE=/absolute/path/to/conan \
  cargo test --locked --test conan_real_tests -- --ignored
```

## 许可证

采用 [Apache License 2.0](LICENSE)。

## 贡献

保持修改范围聚焦。可观察行为的变化应附回归测试，提交前运行上述检查；用户契约有变化时同时更新两种 README。CI 只验证当前 crate 自身。

## 作者

由 [Qubit 组织](https://github.com/qubit-ltd)维护。
