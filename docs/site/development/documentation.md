# 维护文档网站

文档站使用 VitePress，内容随 Gamer 仓库维护。部署产物是静态 HTML，不需要启动 Gamer 服务端。

## 本地编写与预览

在仓库根目录执行：

```powershell
pnpm --dir docs install --frozen-lockfile
pnpm --dir docs dev
```

打开终端显示的地址，默认是 `http://127.0.0.1:5174/gamer/`。正式构建和预览：

```powershell
pnpm --dir docs build
pnpm --dir docs preview
```

预览默认端口为 4174。默认使用仓库子路径 `/gamer/`，与 GitHub Pages 保持一致。

## 内容放在哪里

- `docs/site/`：对外发布的主页、使用手册、开发指南和参考入口。
- `docs/.vitepress/`：导航、侧栏、搜索、主题与构建配置。
- `docs/site/public/`：文档网站专用公共资产。
- `docs/reference/YAML.md`、`docs/reference/KEYMAP_SCHEMA.md`、`docs/guides/yaml-tutorial.md`：由站点包装页直接包含，修改权威原文即可更新网站。

网站源目录只指向 `docs/site/`。开发计划、报告、验收截图和本机数据不会因为存在于 `docs/` 就自动发布或进入搜索索引。新增页面时同时补侧栏与站内链接，检查示例是否符合当前实现。

## GitHub Pages

仓库中的 `.github/workflows/docs.yml` 会在文档相关的 pull request 上构建检查；推送到 `main` 后构建并发布，也支持手动触发。

首次部署需要在 GitHub 仓库 **Settings → Pages → Build and deployment → Source** 选择 **GitHub Actions**，并将文档源码和工作流提交到默认分支。工作流成功后，以部署任务返回的 URL 为准。

默认项目站地址按仓库组织为 `https://<owner>.github.io/<repository>/`。fork 后基路径从 `GITHUB_REPOSITORY` 自动计算；使用自定义域名并部署在根路径时，给构建设置 `DOCS_BASE=/`。

## 发布前检查

运行生产构建，确保没有死链接；在预览中检查首页、搜索、侧栏、移动端菜单和代码复制。发布只上传 `.vitepress/dist/`，不上传源码目录或整个工作区。
