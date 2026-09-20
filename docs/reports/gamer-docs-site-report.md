# Gamer 官方文档站

日期：2026-09-20。

## 交付

- 生产地址：https://jesongit.github.io/gamer/
- VitePress 1.6.4，21 个页面：入门、安装、设备、工作台、自动化、模板、映射、视频、任务、配置与插件、排障、开发和参考。
- 深灰/黄色主题，支持明暗切换、中文全文搜索、移动端目录、代码复制和页内导航。
- 源码在 `docs/site/`，构建配置在 `docs/.vitepress/`，独立依赖和锁文件在 `docs/`。
- YAML、按键映射语法与 YAML 教程通过 include 复用已有权威文档，不复制维护。
- README 增加官网入口；站点不收录开发计划、报告和本机测试数据。

## 发布与后续维护

首次发布只将生成的静态文件提交到独立 `gh-pages` 分支，提交 `6e61fd7`；没有提交或推送工作区中其他未提交改动。GitHub Pages 首次部署运行 https://github.com/jesongit/gamer/actions/runs/35487792058 成功。

首次部署后已将 Pages 构建方式切换为 GitHub Actions，现有生产网站仍正常服务。`.github/workflows/docs.yml` 在相关 PR 上构建，在 main 推送文档变更后构建并发布。

源码与工作流随本次项目整理提交到 main；推送后由 Documentation 工作流接管后续发布。`gh-pages` 只保存首次发布快照，不作为 Markdown 编辑源。网页的“在 GitHub 上编辑此页”指向 main 中对应的 Markdown 源码。

维护流程见 `docs/site/development/documentation.md`。内容以当前开发版本为基准，尚未提供按发行版本切换的文档。

## 验证

- 锁文件安装和生产构建通过；没有构建死链接或 SSR 错误。
- 本地预览和生产地址均使用真实 Chrome 检查全部 21 个页面、中文搜索“虚拟屏”、移动端菜单和窄屏溢出。
- 人工查看 1920×1080 桌面与 390×844 移动端截图；修正首页标题断行，确认侧栏收起后正文正常显示。
- Git diff 空白检查通过。

自动化检查与截图位于忽略目录 `server/target/docs-*`，不是站点发布内容。VitePress 配置函数序列化与 preview 重启事项已记录到 `docs/PITFALLS.md`。
