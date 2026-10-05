# Codex 真机验收启动拒绝：执行平台排查（2026-09-25）

## 结论

已确认本次拒绝发生在命令创建进程之前。任务有效权限为 `never` + `danger-full-access`，桌面应用解析的审批者为 `user`。本机 Codex 执行器内存在与返回一致的命令策略拒绝错误模板，证据支持定位到 Codex 命令执行策略检查；**具体命中规则、内置检查或拒绝理由仍未暴露，不能确认根因或宣称已修复**。

此前把它笼统称为“自动审批拒绝”；本次没有找到 Guardian/Auto-review 对该调用的决策记录，后续应使用“执行策略拒绝”的准确表述。功能列表包含 Guardian 并不能证明该次调用由 Guardian 拒绝。

本轮仅排查并整理诊断材料，没有重试被拒命令，没有改执行工具、包装、审批或安全配置，没有执行 ADB 或启动 Gamer。

## 精确关联信息

| 字段 | 值 |
| --- | --- |
| 拒绝时间 UTC | `2026-09-25T07:38:45.869Z` |
| 拒绝时间北京时间 | `2026-09-25 15:38:45.869` |
| Task / thread ID | `01a0d77a-f4fd-7883-bd39-077589de8556` |
| Turn ID | `01a0d77b-053b-7e11-8818-54d4ddb1247b` |
| Tool call ID | `call_jp5rhMHRHc4gwo7dsXJI4O8i` |
| Code-mode cell ID | `12` |
| 桌面 Appx 包版本 | `26.917.9434.0` |
| 桌面日志 release | `26.917.71314`（与 Appx 包版本字段不同） |
| 实际运行的 Codex CLI | `0.155.0-alpha.16.4` |
| 执行器目录 | `%LOCALAPPDATA%/OpenAI/Codex/bin/13995fba801849b0/` |
| 工作目录 | `D:/code/gamer` |
| 工具返回 | `exec_command failed: CreateProcess ... rejected: blocked by policy` |

任务原始返回报告 `Wall time 0.0 seconds`；本地工具分发日志的总耗时为 87ms，code-mode host 耗时约 13.3ms。分发日志 `execution_started=true` 指工具处理器开始执行，**不能解释为被拒的 PowerShell 或 Gamer 子进程已经启动**。

## 已核实证据

1. `D:/AppData/codex/config.toml` 中 `approval_policy="never"`、`sandbox_mode="danger-full-access"`。当前任务 `turn_context` 同样记录这两个有效值；桌面日志在 `07:32:38.649Z` 明确记录 `resolvedApprovalPolicy=never`、`resolvedApprovalsReviewer=user` 和请求的 `:danger-full-access` 权限档。
2. `C:/Users/Jeson/.codex` 是指向 `D:/AppData/codex` 的 junction，两个位置不是两套独立配置。检查到用户 `rules/` 不存在，项目 `D:/code/gamer/.codex` 不存在；未在检查的父目录或 `%ProgramData%/OpenAI/Codex` 等标准位置找到 `requirements.toml` / `managed_config.toml`。**这不能排除内置策略或服务端下发的限制。**
3. 本机 `codex.exe` 的只读字符串检查发现完整错误模板 `` `…` rejected: blocked by policy ``，位于包含 `.rules files`、`requires approval by policy`、`policy forbids commands starting with` 的字符串区域。它支持命令策略层定位，但字符串相邻关系不能证明具体判断分支或触发词。
4. 原始任务记录完整保存了被拒的工具输入；拒绝返回中的命令文本已由工具截断，未找到完整拒绝原因。仅提取本次失败调用与对应权限记录，未复制完整会话。
5. 当天桌面日志及 `logs_2.sqlite` 中未找到这次调用对应的 execpolicy/Guardian 决策或规则说明；数据库记录了工具调用分发和耗时。已排除搜索命中“读取拒绝文档”“搜索策略日志”等工具输入的情况，避免将会话复述误当成执行器诊断。
6. 查询北京时间 `15:37:30–15:39:30` 的 Windows Code Integrity、AppLocker EXE/DLL、AppLocker MSI/Script 日志，均返回 `NoMatchingEventsFound`；只能说明这些日志中没有匹配事件，不能证明所有 Windows 安全组件均未参与。
7. 拒绝后既有只读检查未发现测试进程、相关端口监听或 `qa-*` 结果。文件读取、脚本编辑、Python 静态编译及普通查询均可执行，故不能把问题概括为整个 PowerShell/文件系统执行环境不可用。

## 单独发现的沙箱问题

`D:/AppData/codex/.sandbox/setup_error.json` 记录：

```text
helper_sandbox_lock_failed
SetNamedSecurityInfoW sandbox dir failed: 5
```

当天沙箱日志显示错误发生于 **13:31:34**，对应另一条 `git statu` 命令的沙箱设置刷新；随后有 `read-acl-only mode` 和 `read ACL run completed`。该记录早于本次 15:38:45 拒绝，且本次任务使用 `danger-full-access`，没有证据连接二者。未修改 ACL、删除沙箱目录或重启沙箱服务；不把修复该独立问题当成本次拒绝的已知解决办法。

## 诊断材料与后续处理

精简原始材料保存在 `backups/beta-release/codex-policy-diagnostics-20260925/`：

- `denial-original.txt`：从任务记录直接提取的拒绝返回，保留原有截断。
- `denial-metadata.json`：精确时间和调用关联 ID。
- `rejected-tool-input.txt`：被拒工具输入的原文，仅作诊断文本，未再次执行。
- `effective-turn-context.json`、`permission-config-only.json`、`app-permission-log.txt`：三处权限证据，仅保留相关字段。
- `tool-dispatch-log.json`：本次调用的两条分发/耗时记录。
- `unrelated-sandbox-error.json`：单列的早先沙箱错误，不作为根因证据。

这些文件未包含 `auth.json`、`.env`、admin-token 内容或完整会话；仍包含本地路径、任务 ID 和原始命令。材料仅保存在本地，未上传或发送反馈。

下一步需 Codex 执行平台按上述 call/turn/thread ID 查询该命令策略评估：实际 decision、命中规则或内置检查、配置来源，以及为何完整访问任务只收到通用拒绝信息。请求修复误拒或补足正式审批/诊断路径，不通过改包装、关闭防护或换执行通道规避。

可通过应用输入框 `/` 的反馈入口，附本报告与精简材料；[官方排障文档](https://learn.chatgpt.com/docs/reference/troubleshooting) 说明可从当前任务提交反馈并选择附带会话，提交后取得供团队关联的 session ID。本次尚未提交反馈或创建工单。

[官方命令规则文档](https://learn.chatgpt.com/docs/agent-configuration/rules) 说明 `forbidden` 会直接拒绝、不弹审批；这解释了明确用户授权与无审批弹窗可以并存，但**不能证明本次确实命中了某条 `forbidden` 规则**。
