# Gamer 安装、修复与升级

当前启动器提供图形安装/更新、托盘运行和命令行维护。正式发行版为 [Gamer 0.2.0](https://github.com/jesongit/gamer/releases/tag/v0.2.0)，支持独立启动器在线安装和完整包离线安装。使用入口见 [启动器快速上手](launcher-quickstart.md)，正式发行验收范围见 [0.2.0 记录](../evidence/RELEASE_0_2_0_2026_09_25.md)。

## 安装和日常使用

1. 将完整包解压到可写目录，双击 `gamer-launcher.exe`。
2. 首次点击“安装”，从包内 seeds 安装本体与锁定的 ADB、FFmpeg/FFprobe、scrcpy-server。
3. 首次选择官方插件并设置管理员密码；个人配置包在软件中导入。

正常启动在后台完成检查，进入托盘并打开网页；发现更新则显示小窗口，提供“更新 / 取消”。托盘右键仅“打开 Gamer / 退出 Gamer”。运行数据在 `data/`，配置在 `config/`；这些目录不随本体升级替换。

0.2.0 支持在“设置 → 软件更新”中检查、下载并安装已确认的版本，安装失败自动恢复旧版和升级前快照；beta.4 及更早启动器须先通过启动器更新到 beta.6。插件页独立发现官方插件新版本，安装或更新仍由用户确认。

不要将新版完整包直接覆盖已有安装的 `config/` 或 `data/`。离线升级先退出启动器并备份这两个目录，再将新包的 `seeds/`、`manifests/` 合并到原安装目录，并替换根目录 `gamer-launcher.exe`；随后使用下方 `upgrade --manifest` 指定新版清单。完整包的新配置模板仅供首次安装使用。

## 完整性与来源

发行清单默认从官方 GitHub HTTPS 下载，按 size/SHA256 校验归档和组件文件，再安全解压。
不需要 `.sig`、公钥、密钥轮换或签名 secrets。SHA256 保证文件与清单一致；清单来源依赖 GitHub 账号与发布权限，不能用 SHA256 认证发布者。

离线种子和下载缓存同样校验。下载可中断恢复，校验缓存可复用未变化文件的校验结果，变化后重新计算。需要强制全量哈希检查时使用 `doctor --deep`。

## 命令行维护

在安装目录运行：

```powershell
.\gamer-launcher.exe status
.\gamer-launcher.exe doctor
.\gamer-launcher.exe doctor --deep --probe
.\gamer-launcher.exe doctor --manifest manifests/<版本>.json
.\gamer-launcher.exe repair --probe
.\gamer-launcher.exe upgrade --manifest <新版本清单路径或HTTPS地址>
```

`repair` 用已有发行清单从 seeds/cache/远端修复；`upgrade` 要求明确候选清单。
`start` 用于命令行监管服务；日常双击使用图形入口。
没有 CLI `stop` 子命令；日常退出使用托盘“退出 Gamer”。

## 安装目录

| 目录 | 用途 |
|---|---|
| `versions/<版本>/` | 应用与 Web 静态资源 |
| `runtime/<组件>/<版本>/` | 锁定的运行依赖 |
| `manifests/` | 已校验发行清单 JSON |
| `seeds/`、`cache/`、`staging/` | 离线归档、下载缓存、安装暂存 |
| `state/` | 版本指针、安装库存、升级 journal |
| `backups/`、`quarantine/` | 升级快照与故障现场 |
| `data/`、`config/` | 用户数据与配置 |

升级异常时保留 journal、快照与 quarantine，先关闭仍占用旧版本目录的进程再修复。
`manifest_invalid` 表示清单结构/语义错误，`artifact_invalid` 表示文件哈希、大小或归档检查失败。
自动回滚也失败会进入 `manual_recovery_required`，按 [发布与人工恢复手册](RELEASE.md) 处理。

维护者使用 [安装目录契约](UPDATE_CONTRACT.md)、[发行清单契约](../../release/contracts/manifest-v1.md) 和 [发布资产验证](../../release/docs/ATTESTATION.md)。旧计划与签名演练记录仅作为历史资料。
