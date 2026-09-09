# Gamer 全插件正确性与交互优化：P6-SERVER 验收证据

> 验证日期：2026-09-09
> 基线提交：`b242c0d586c19bb0fdb996e05879e3f0459ef9ff`
> 范围：服务端生命周期实现、已有 Rust 契约测试、独立生命周期回归测试
> 状态：本证据仅记录当前工作树的服务端范围，不代表全项目验收完成

## 本次实现

- `server/src/extensions/service.rs`：Running 插件更新/卸载自动停止并恢复；更新失败保留旧版本；新版本启动失败回滚 active 版本；必需依赖守卫继续在停止边界前执行；无替代版本时卸载最后一个 Running 版本直接完成，不虚构恢复失败。
- `server/src/extensions/service_m01_tests.rs`：补充 Running/Disabled 更新、失败恢复、替代版本卸载、幂等更新，以及最后一个 Running 版本卸载回归覆盖。
- 未修改前端、`server/src/api/mod.rs`、计划文件或 `server/data/`。

## 实际命令与结果

命令均在 `E:\code\gamer\server` 执行，结果如下：

| 命令 | 结果 |
|---|---:|
| `cargo fmt --all -- --check` | 退出码 0 |
| `cargo check --all-targets` | 退出码 0 |
| `cargo test extensions::service --all-targets` | 38 passed, 0 failed |
| `cargo test resources --all-targets` | 18 passed, 0 failed |
| `cargo test architecture_guard --all-targets` | 7 passed, 0 failed |
| `git diff --check`（仓库根目录） | 退出码 0 |

## 覆盖结论

- 更新：Running 更新停止一次并恢复 Running；Disabled 更新保持 Disabled；重复同版本更新在停止前冲突；安装失败和新版本启动失败均保留旧 active 版本。
- 卸载：Running 活跃版本在存在替代版本时恢复替代版本；最后一个版本卸载成功且不尝试启动不存在的替代版本；非活跃版本卸载不打断当前运行实例。
- 依赖：必需依赖缺失、版本不兼容、循环和被运行中消费方引用的 provider 停用/卸载均有门禁；可选依赖不阻塞消费方。
- 资源：资源级 `expected_version` 冲突、Package manifest `expected_revision` 门禁、插件目录隔离和并发完整写入均通过已有 Rust 测试。
- 架构：7 项 Core/Extension/YAML/Keymap 边界测试全部通过。

## 失败分类与未验证项

- 本次最终范围内没有失败测试或格式/编译失败。
- 曾启动 `cargo test --all-targets -- --nocapture`，随后按用户要求中断当前全量观察；该全量命令没有保留可引用的完整退出结果，故全量 Rust 测试套件标记为 `NOT_VERIFIED`，不能据此宣称全量通过。
- 真实浏览器、Android 设备、WebRTC、真实 WASM/builtin 插件包更新/卸载、录制会话和跨进程恢复未验证，均为 `NOT_VERIFIED`。
- 多进程/磁盘故障注入下的管理操作原子性未验证；现有测试覆盖的是服务内可控失败路径。
