# 官方插件分仓验收（2026-09-24）

## 布局与来源

- 插件仓：https://github.com/jesongit/gamer-plugins，迁移基线 `689cc47:plugins/`，迁移前历史继续保存在主仓。
- 主仓迁移分支：`codex/plugin-repository-split`，plugins/ 为固定提交 submodule，不跟随远端 main。SDK 宿主基线 `9abe63b294ed54764c52ceb564a0724696b5612b` 已在远端可获取。
- SDK 包含 7 个构建文件（WIT、UI 桥与模块表、打包器源码和 Cargo.lock），lock.json 固定来源提交与逐文件 SHA256；主仓门禁检查快照与当前接口一致。
- 主仓构建入口委托插件仓，web build 只构建壳，发行流水线先显式构建插件。插件 Release 以插件自己的版本 tag 触发，先跑测试，再创建草稿；本轮没有发布 tag、正式 Release 或切换线上市场源。

## 本机验证

| 检查 | 结果 |
| --- | --- |
| 独立插件构建（隔离目录，无相邻 Gamer 源码） | 3 个 .gplugin、registry 与 SHA256 清单通过打包自检 |
| 主仓构建包装入口 | 向临时输出目录成功构建 3 个包，不改动既有市场归档 |
| 服务端 cargo test --locked | 703 通过、6 个环境/性能测试保持默认忽略 |
| Web 与插件 UI | 773 + 243 + 7 = 1,023 通过；Keymap 无测试文件，不计覆盖 |
| 独立解释器 | 20 通过 |
| 插件发布准备 | 2 通过：tag/manifest 绑定、篡改拒绝与索引哈希重算 |
| Rust fmt / Clippy | 通过（全目标全功能，-D warnings） |
| Web 壳构建 | 通过 |
| 现有发行工作流离线契约 | 通过 |
| 远端全新插件 checkout | 从 GitHub 克隆后完整构建通过，SDK 哈希一致 |
| 主仓远端递归 clone | 成功取回固定子模块提交，SDK 主从校验通过 |

插件仓 CI 已触发 Windows 独立构建和 Linux 固定宿主集成，远端结果单独记录，不以本机结果替代。

## 数据与备份

迁移前 plugins/ 完整目录（包括 ignored 构建缓存）保留于 `backups/plugin-split-20260924/original-plugins`。现有 config/data/state、发行安装目录及历史 web/public/plugins/*.gplugin 未改动。新的独立构建产物在插件 dist/ 或临时输出目录；未重新生成用户的完整运行包。

日志位于系统临时目录，前缀 gamer-split-，包括 build、main-build、remote-build、server、web、web-build、clippy、interpreter、release。

## 仍待发行

主仓迁移分支合并、生产签名配置和正式版本资产发布、首个插件 Release 的真实下载/安装验收后再切换在线目录。干净 Windows、真实 Android、断电等之前未验收的外部场景仍保留在总计划。
