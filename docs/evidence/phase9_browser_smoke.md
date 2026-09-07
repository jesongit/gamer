# Phase 9 浏览器实机冒烟（集成者执行）

日期：2026-09-07 ｜ 隔离环境：后端 18443（临时 config+数据目录，GAMER_ADMIN_PASSWORD 进程内生成）+ Vite 5174（VITE_PROXY_TARGET 指向 18443）；8443 上的用户旧实例未触碰。基线 = db4d481 + 冒烟中发现的两个修复。

## 结论

PASS（发现并修复 1 个存量功能回归 + 1 个展示缺陷，均在本轮收口）。NOT_VERIFIED 仅剩真机 adb 链路与真实 GitHub Release 下载（无设备/无外网发布物，与最终验收报告一致）。

## 实测步骤（真实浏览器 ZCode IAB，1600×1000）

1. 登录页 → admin/开发密码 → 进入 `#/console?panel=gamer.core:tasks`。✅
2. 主导航「市场▾」下拉 → 插件市场/配置市场两分区。✅
3. 打开插件中心市场：三插件齐（keymap 1.0.1 WASM / video 1.0.0 宿主预置（需要 Gamer 宿主支持）/ yaml 3.1.1 WASM），来源=官方市场、权限清单、固定版本 SHA-256 口径、界面无签名/proof 概念。截图 `phase9_smoke_market_plugins.png`。✅
4. 安装 gamer.video：下载 → inspect → 确认弹窗（ID/版本/来源/执行形态/权限增量）→ 安装成功。yaml 同链路复验（埋桩自动确认），POST /api/extensions 201。✅
5. 安装后「插件▾」下拉出现面板项（先 视频，装 yaml 后 视频/自动化/函数/模板）。✅
6. 打开 `gamer.video:video` 面板：VideoWorkbench 三分区（素材库/项目/草稿）正常渲染，缺 Package 时显示可诊断横幅（截图 `phase9_smoke_video_workbench.png`）。✅
7. 打开 `gamer.yaml:automation`：ScriptRunner 正常。✅

## 冒烟发现并已修复

1. **业务面板激活回归（P1，基线遗留下拉菜单改造引入）**：`PluginWorkspace.activeTop` 把所有非 core key 兜底成 `'plugins'`，`selected` 随之恒为 null → 业务面板（视频/自动化/函数/模板）点开后永远渲染插件选择器兜底，面板本体无法显示。修复：业务 key 原样透传给槽位渲染，主导航页签高亮经新增 `activeTab` 映射回「插件」，二级导航条件同步放宽；新增 `web/src/workspace-plugin-panel.test.js` 3 项回归锁定（业务面板渲染/选择器视图/core 路径不回归）。
2. **市场卡片 UI 类型标签失真（P3）**：`uiType()` 把 `runtime="core"` 显示成 `declarative`——补 core 档位映射为「core（宿主组件）」。

## 测试特异现象（非产品缺陷）

首次通过 IAB 原生 confirm 桥接接受安装确认后，安装 POST 疑似未发出（fetch 挂起、busy 卡死）。以页面内自动确认复验同链路全通（yaml POST 201），且 curl 直连安装端点瞬时成功——判定为无头浏览器 JS 对话框桥接的测试特异现象；真实浏览器 confirm 同步返回不受影响。另：本应用对 Playwright 指针点击存在拦截（登录/页签均超时），自动化一律走页面内 `.click()`，与产品无关。

## 测试

`pnpm test:run`：65 文件 / 813 tests 全绿（含新增 3 项）。服务端无改动。
