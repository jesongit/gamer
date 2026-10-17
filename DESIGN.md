---
name: "Gamer"
description: "炭灰、暖白与少量橙黄构成的紧凑游戏工作台"
colors:
  bg-0: "#17191c"
  bg-1: "#202226"
  bg-2: "#282a2f"
  bg-3: "#34363b"
  chrome: "#111315"
  field: "#191b1f"
  border: "#3b3d42"
  control-border: "#7d7b75"
  text-0: "#eeece6"
  text-1: "#cecac1"
  text-2: "#b6b6ae"
  accent: "#dda951"
  accent-2: "#edbc6c"
  warn: "#f0cb7f"
  danger: "#ed9696"
  ok: "#8fc9ae"
  accent-ink: "#201d16"
typography:
  brand:
    fontFamily: "'Gamer Brand', sans-serif"
    fontSize: "24px"
    fontWeight: 700
    lineHeight: 1
    letterSpacing: "-0.02em"
  display:
    fontFamily: "'Gamer Heading', 'Microsoft YaHei', sans-serif"
    fontSize: "33px"
    fontWeight: 600
    lineHeight: 1.3
  title:
    fontFamily: "'PingFang SC', 'Microsoft YaHei', 'Segoe UI', system-ui, sans-serif"
    fontSize: "19px"
    fontWeight: 600
  navigation:
    fontFamily: "'PingFang SC', 'Microsoft YaHei', 'Segoe UI', system-ui, sans-serif"
    fontSize: "16px"
  body:
    fontFamily: "'PingFang SC', 'Microsoft YaHei', 'Segoe UI', system-ui, sans-serif"
    fontSize: "14px"
  label:
    fontFamily: "'PingFang SC', 'Microsoft YaHei', 'Segoe UI', system-ui, sans-serif"
    fontSize: "13px"
  assistant-guide:
    fontFamily: "'PingFang SC', 'Microsoft YaHei', 'Segoe UI', system-ui, sans-serif"
    fontSize: "15px"
    lineHeight: 1.7
  assistant-input:
    fontFamily: "'PingFang SC', 'Microsoft YaHei', 'Segoe UI', system-ui, sans-serif"
    fontSize: "15px"
    lineHeight: 1.6
  mono:
    fontFamily: "'Cascadia Code', 'JetBrains Mono', Consolas, 'Courier New', monospace"
rounded:
  sm: "3px"
  default: "4px"
spacing:
  s4: "4px"
  s6: "6px"
  s8: "8px"
  s10: "10px"
  s12: "12px"
  s16: "16px"
  s24: "24px"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "5px 10px"
  button-primary-hover:
    backgroundColor: "{colors.accent-2}"
    textColor: "{colors.accent-ink}"
  button-secondary:
    backgroundColor: "{colors.bg-3}"
    textColor: "{colors.text-0}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "5px 10px"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.text-1}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "5px 10px"
  button-toolbar:
    backgroundColor: "{colors.bg-3}"
    textColor: "{colors.text-0}"
    rounded: "{rounded.sm}"
    padding: "5px 8px"
    height: "34px"
  button-connect:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
    rounded: "{rounded.sm}"
    padding: "8px 28px"
  field:
    backgroundColor: "{colors.field}"
    textColor: "{colors.text-0}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "3px 8px"
  plugin-selector:
    backgroundColor: "{colors.field}"
    textColor: "{colors.text-0}"
    typography: "{typography.body}"
    rounded: "{rounded.sm}"
    padding: "4px 8px"
    height: "34px"
  navigation:
    backgroundColor: "{colors.chrome}"
    textColor: "{colors.text-1}"
    typography: "{typography.navigation}"
    padding: "8px 17px"
  tag:
    backgroundColor: "{colors.bg-3}"
    textColor: "{colors.text-1}"
    rounded: "{rounded.sm}"
    padding: "1px 5px"
  card:
    backgroundColor: "{colors.bg-1}"
    textColor: "{colors.text-0}"
    rounded: "{rounded.default}"
    padding: "12px"
  workspace-panel:
    backgroundColor: "{colors.bg-1}"
    rounded: "{rounded.sm}"
    padding: "0"
  assistant-input:
    backgroundColor: "{colors.field}"
    textColor: "{colors.text-0}"
    typography: "{typography.assistant-input}"
    rounded: "{rounded.sm}"
    padding: "8px 10px"
---

# Design System: Gamer

## Overview

2026-10-16 产品职责更新：按已确认的外部 Agent 改造计划移除内置聊天、模型设置与 AI 生成入口。本文旧助手示例仅记录已经确认的视觉规则，不要求保留已退休的功能。继续保留当前分屏、导航、配色、可读字号和控件密度；MCP 接入置于现有设置，自动化保留工作稿与版本、历史候选及运行日志。

**Creative North Star: "大画面，紧凑工作台"**

以“大画面，紧凑工作台”为视觉中心：用分层炭灰承接游戏画面与工具，用暖白建立清楚的阅读层次，橙黄只承担主要动作、选中位置和必要状态。明日方舟官网是用户指定的风格参考；实际结构由 Gamer 的操作目标和插件能力决定。当前实现采用细线边界、近直角的小圆角、低装饰密度，不继承重设计前的视觉组织。

界面面向不熟悉脚本的普通玩家。品牌字与断线标题使用本地字体子集；日常中文、对话和控件使用系统 UI 字体。紧凑来自合并重复标题、按需展开次要工具和减少外层留白；不能把较小字号或难点的点击区域作为空间来源。这里记录当前 Console 及其共用原语，未纳入本轮的旧页面样式不自动成为新系统的规则。

记录与验收边界：当前截图和源码已完成独立 finish reviewer 所列具体 UI 修正，该 ship 结论仅覆盖这份修正清单。hero 构建阶段仍为 OPEN，用户已明确确认大体结构与风格，具体功能与真实环境效果分别验证；组件计划的真实本地批准回执不能替代首屏验收。原生 comp-diff 存在像素错位／虚构白区缺陷，独立原图裁切只作诊断，不产生相似度或批准。没有取得 QUALITY BAR 卡片；本地 FORM 记录已核实，远端目录来源未取得。预览目标与配置为临时示例，真实设备、游戏、模型和 ICE 效果未验收。本轮重设计未发布生成位图，品牌几何与首屏图标来自 CSS／SVG。

**Key Characteristics:**

- 深灰分层、暖白文字，橙黄集中于动作与选中状态。
- 近直角细线框与紧凑工具条，正文和输入保留可读尺寸。
- 工作台的大画面与可调插件区；全局页面复用同一视觉原语。
- 插件名称及功能来自真实贡献，尺寸变化和页面切换保留会话草稿。
- 空态诚实说明缺失条件，并给出明确恢复动作。

## Colors

色值以 frontmatter 为准，名称沿用源码变量，避免出现第二套颜色权威。

### Primary

- **暖橙黄 `accent`**：连接画面、运行、发送等主要动作，以及导航下划线、输入焦点和分隔条抓手。
- **亮橙黄 `accent-2`**：主要动作 hover 和较亮的状态提示。
- **深色强调文字 `accent-ink`**：橙黄实心按钮与选中文字的前景色。

### Neutral

- **底层炭灰 `bg-0`**：页面底色及未连接的游戏画面区域。
- **工作区深灰 `bg-1`**：插件工作区、对话底部与通用容器。
- **浮层灰 `bg-2`**：菜单、选中插件页签、模型条件行及展开内容。
- **控件灰 `bg-3`**：次要按钮和 hover 背景。
- **顶栏黑灰 `chrome`**：应用顶栏和游戏目标工具条。
- **输入黑灰 `field`**：选择器、输入框与助手消息输入。
- **细线灰 `border`**：区域边界、工具条分隔和普通按钮边线。
- **控件边线灰 `control-border`**：输入、选择器和可拖拽抓手；其对比高于普通区域分隔。
- **暖白 `text-0`**：主要文字；**次级暖灰 `text-1`**：次要内容；**提示灰 `text-2`**：帮助、占位和元信息。

### Status

- **提示黄 `warn`**、**错误浅红 `danger`**、**成功柔绿 `ok`**只表达实际状态，不取代主要品牌强调色。信息／运行标签的前景和浅底复用橙黄。

### Named Rules

**The Sparse Accent Rule.** 橙黄用于可执行的主要动作、选中导航、焦点和运行提示；普通容器保持中性深灰，不铺满高饱和强调色。

## Typography

**Display Font:** 本地 `Gamer Heading`（Noto Sans SC 600 子集），仅用于当前断线标题；回退为 Microsoft YaHei 与 sans-serif。  
**Brand Font:** 本地 `Gamer Brand`（Quicksand 700 子集），用于 Gamer 标识；斜体来自组件 CSS。  
**Body Font:** PingFang SC、Microsoft YaHei、Segoe UI、system-ui、sans-serif。  
**Label/Mono Font:** Cascadia Code、JetBrains Mono、Consolas、Courier New、monospace；仅用于源码、标识和相应参数内容。

专用字体形成有限的品牌层次；中文操作说明使用熟悉的系统字体。不同角色分别服务品牌、状态、导航和阅读，不以一种展示字体覆盖整个工具。

### Hierarchy

- **Brand**：桌面斜体（700，24px，行高 1，字距 -0.02em）；小屏品牌字（21px）。
- **Display**：未连接标题（600，33px，行高 1.3）；800px 以下为 23px。真实连接错误改用系统字体（18px），不将长错误渲染成展示标题。
- **Title**：助手空对话标题（600，19px）。
- **Navigation**：桌面全局导航（16px），选中为 600；插件选择和 AI 自有页签（14px）；宿主多贡献子页签（13px）。
- **Body**：页面基础文字（14px）；助手空态提示（15px，行高 1.7）、消息输入（15px，行高 1.6）、模型条件行（14px）。消息正文遵循实际业务内容的排版，不用展示字号代替阅读字号。
- **Label**：一般按钮和紧凑操作（13px）；状态标签（12px）。更小的旧诊断元数据不作为新页面的排版基线。

### Named Rules

**The Readable Density Rule.** 先合并控件与减少重复留白，再收紧布局；保留当前助手页签／警告的 14px、提示／输入的 15px，触控尺寸按已实现的粗指针规则处理。

## Layout

工作台在桌面使用游戏画面与插件工作区的横向分栏；这是一项当前表面的空间规则，不要求任务、运行记录、配置包等全局页面也分栏。全局页复用顶栏和视觉原语，内容占满工作区。

- 应用顶栏（48px）容纳品牌、六项主导航及账户动作。上下文工具条最小高度（46px），配置包选择器在工作台和所有全局页面常驻，收起插件后仍可切换；全局页面省去游戏目标和操控按钮。工作台的“游戏操作”展开应用、网页和其他控制工具。
- 工作台外边距为上下 6px、左右 8px，画面与插件区之间为 10px 分隔条。游戏画面按原始比例 `contain`，不使用装饰插画或固定比例内边距。
- 插件区初始比例（0.32），普通分栏最大为可用宽度的一半，最小为 320px 与该最大值两者中的较小值。调整比例保存到本地；键盘左／右每次改变 20px，方向与拖动一致。
- 插件头部最小高度（44px），容纳有名称的选择器及“放大／还原”。“收起插件／展开插件”是顶部工具条同一个按钮的两个状态，位置不变；放大占满内容区，收起让画面取得空间。尺寸动作不停止后台运行。
- 插件内容由插件负责内边距，宿主不叠加统一留白。自动化、视频与键盘映射工作区使用 `--workspace-inset: 8px`，表单和可滚动编辑器再保留 2px 焦点间距；助手已有 8px、声明式面板已有 10px 的边距保持。模板筛选和动作按整组换行，编辑器和媒体内容在自身区域内纵向滚动，避免底部控件被裁切。
- 800px 以下按画面、插件区纵向排列：外边距 4px、间隔 6px，画面为 38vh 且至少 230px，插件区至少 460px；隐藏横向拖拽分隔条。允许纵向滚动，不把全部功能挤进一屏。
- 700px 以下顶栏换行，主导航单列横向排列并可滚动；次要账户文字隐藏。1000px 以下收紧目标选择器宽度。粗指针设备对目标工具条、助手动作和发送等已指定控件采用至少 44px 点击高度。
- 助手对话区使用剩余高度并独立滚动；条件提示与输入区保持在工作区底部。次要设置和预算通过“更多”展开。桌面／移动捕获分别为 1586×992、390×844；这些尺寸是证据样本，不是新的断点。

空间节奏以已重复使用的 4、6、8、10、12、16、24px 为基准：小间隔给关联控件，较大内边距给内容与全局页面。

## Elevation & Depth

静态工作台依靠背景层次和细线边界建立深度。没有给每个分区添加阴影；操作菜单、原生选择器浮层、弹窗和设置抽屉才使用柔和遮罩或阴影。游戏识别／操作证据的局部发光用于说明实际状态，不是容器装饰。

### Shadow Vocabulary

- **浮层阴影**（`0 8px 24px rgba(0, 0, 0, .35)`）：共用菜单、弹窗、toast 与选择器浮层。
- **助手设置抽屉**（`-10px 0 30px #0003`）：从右侧覆盖工作区，配合左边线。
- **助手更多菜单**（`0 8px 24px #0005`）：当前插件菜单的局部实现；不替代共用阴影。

通用状态过渡为 150ms 的文字／背景／边线变化；开关为 200ms，运行圆点以 1.2s 周期改变透明度。系统要求减少动态效果时，动画与过渡压到 0.01ms，并取消平滑滚动。

### Named Rules

**The Flat Workspace Rule.** 工作台分区靠背景层次和细线分隔；阴影只帮助识别浮层、菜单与抽屉，不给每个工具容器增加悬浮感。

## Shapes

以近直角的小圆角容器为主：输入、按钮、工作台边框使用小圆角（3px）；通用卡片、弹窗与分隔条抓手使用标准圆角（4px）。圆形仅承担状态圆点、滑杆和既有专门控制，不把主面板做成胶囊卡片。

边界通常为 1px。主导航与功能页签用下边线（2px）标明选中；重设计首屏的 Gamer 双斜线标识由 CSS 几何绘制。共用动作图标来自 `UiIcon` 的真实 SVG 路径（24×24 viewBox、1.7 描边、通常 16px 显示），图标操作具有名称、title 或 aria-label。

## Components

### Buttons

紧凑、直接，以状态表达可执行性。

- **Primary**：橙黄底配深色字；hover 用亮橙黄。一般原语为 3px 圆角、5px 10px 内边距、至少 32px 高。目标工具条覆盖为 34px 高及 5px 8px 内边距。
- **连接画面**：断线空态的主要下一步，至少 44px 高／200px 宽，8px 28px 内边距、16px 字。
- **Secondary / Ghost**：控件灰实底与透明底；hover 调整边线和文字，ghost 可获得控件灰底。
- **Danger / Disabled**：危险动作使用错误色的文字与边框；禁用透明度 0.5，保持禁用语义与不可点击状态。
- **Focus**：可见 2px 橙黄描边，偏移 2px。触控高度按 Layout 中已实现的范围提升，不把全仓旧控件声明为统一 44px。

### Chips

通用标签使用控件灰、次级文字与细线框（3px 圆角，1px 5px 内边距，12px 字）。信息及运行标签用橙黄前景、8% 混色底与 40% 混色边；成功、警告和错误标签保持各自实际状态色。状态不能只由颜色表达。

### Cards / Containers

通用卡片为工作区深灰、1px 细线、4px 圆角和 12px 内边距。工作台左右两个主要区域使用 3px 圆角细线框；插件内容宿主去掉额外卡片边框和重复内边距，让实际插件使用完整空间。全局页仍按自身内容密度处理，不移植工作台的空白比例。

### Inputs / Fields

输入和选择器使用输入黑灰与暖白文字、1px 控件边线、3px 圆角；一般为 3px 8px 内边距、13px 字，目标工具条选择器为 14px 字／34px 高，插件选择器为 14px 字／34px 高。焦点边线变为橙黄，占位用提示灰，光标用橙黄。

原生选择器保留键盘与选择行为；支持 `base-select` 时其弹出列表也使用浮层灰、细线和共用阴影。助手消息输入为 15px／1.6，至少 56px 高，最大 150px；只在已满足真实条件时启用发送。

### Navigation

全局入口为工作台、任务、运行记录、配置包、插件、设置。默认文字使用次级暖灰，hover 为暖白与控件灰底，当前页用橙黄下边线及加粗暖白。

工作台先选择有名称的插件，再展示该插件自己的功能；多贡献插件显示紧凑的可排序子页签，单贡献不重复增加一行。AI 助手内部对话／记忆库／可选服务是插件自身导航，“新对话”和“更多”合并在同一功能行。主导航和插件导航不通过启用／禁用生命周期来切换。

### Plugin Workspace

插件显示名、面板、顺序及 runtime 消费现有 manifest／贡献注册表，支持 core、iframe、declarative，保留应用适用性过滤。每个 `keepAlive: session` 贡献独立缓存；切到全局页或无缓存面板仍保留其草稿，贡献移除时清理对应缓存。运行目标、Android 应用与配置包独立，不由视觉壳互相推导。

分隔条具有可见的 18×56px 中性边框抓手和橙黄 SVG 三线，支持指针与键盘。顶部“收起插件／展开插件”切换可见性，插件头部“放大／还原”调整占用空间；收起的面板保留会话且不改变后台运行。配置包管理页复用顶部选择器，不增加第二个选择器。

### Functional Workflows

用户已确认大体结构与风格。本轮按普通玩家的操作路径改进功能，不更换视觉或导航。

- 脚本库先选择、配置和运行已有脚本。空态提供导入配置包、AI 创建和手动编写；源码、函数与引用按需展开。保存草稿明确为未发布版本，不自动替换脚本库。
- AI 自动化分别解释准备说明与素材、生成版本、检查和测试、发布。版本状态可见；查看和手动编辑源码是进一步操作，原文与发布门禁保留。
- 助手示例只填入输入框，由玩家发送。缺少模型、消息过长和无效预算给出具体提示，首次使用也能看见发送快捷键。暂停、继续、停止及运行授权继续使用原有控制流程。
- 定时任务以频率和时间编辑常见 Cron。复杂表达式保留原值，进入自定义编辑；“立即运行”如实命名真实执行动作，执行功能和参数仍由注册贡献提供。
- 插件管理的“打开功能”只对已启用、适用当前应用的贡献可见，沿用最近查看面板。安装状态与启动未完成分开；ID、来源和权限在详情中提供。
- 模板测试明确相似度和范围，保留命中、未命中或错误结果，切换画面、目标或配置包即清掉旧结果。测试不执行点击，数值 0 如实传入，不当作默认值。
- 视频素材以开始／结束时间截取可用录制范围；无效范围和缺失条件在制作前解释。完整素材给出前往自动化的入口，视频自身不生成脚本，不绑定配置包。
- 失败运行可以直接查看异常动作与判断截图；AI 分析和修复仅在当前配置包满足现有条件时开放。看日志时保留编辑和模板状态，隐藏其内容，给日志完整空间。
- 全局操作日志明确最近记录与搜索范围，清空会影响全部设备，因此操作名称和确认都写明范围；取消确认不发删除请求。

### Honest Empty States

游戏未连接时展示真实状态、连接动作与简短行为说明；关闭画面不代表停止自动化。助手未建立对话时保留可阅读的空态与输入区域；模型未配置时显示 SVG 警告和一个清楚的恢复行，发送按真实条件禁用。示例数据只用于界面预览，不伪装为真实在线或成功结果。

不固化为设计规则：未纳入这次首屏修正评审的高级消息／诊断仍有旧字形图标与 10–11px 元数据；只记录为待单独处理的范围，不供新页面继承。本轮没有修复这些源码，不改变原生 hero 工具门禁，也不代替具体功能与真实效果验收。

## Do's and Don'ts

### Do:

- **Do** 使用 frontmatter 中的现有色值和字体角色；插件通过共用 CSS 变量融入工作台。
- **Do** 优先合并重复标题、模型入口和工具行，保留可读文字以及已实现的触控点击尺寸。
- **Do** 将插件选择、插件自有功能页签与全局插件管理保持为清楚的不同层次；单功能插件省去重复子导航。
- **Do** 在未连接、模型未配置和不可执行状态显示真实条件与恢复入口；保留正常的禁用与焦点状态。
- **Do** 保持游戏图像原比例、现有输入坐标和控制屏障；在切换页面与调整尺寸时保留适用贡献的草稿。
- **Do** 在后续页面复用 CSS 几何标记与真实 SVG 图标，并尊重减少动态效果的系统设置。

### Don't:

- **Don't** 将游戏官网的宣传页结构或大装饰移入操作界面；后续表面延续当前工业感与暖色强调。
- **Don't** 在游戏显示区域添加装饰插画、固定比例内边距，或为了紧凑继续缩小助手提示和输入字号。
- **Don't** 把插件藏成无名称小图标，或在宿主写死官方插件名单、功能和运行能力。
- **Don't** 将配置包当作 Android 应用、连接画面当作启动自动化，或把收起插件区／关闭预览当作停止后台运行。
- **Don't** 将旧高级状态中的字形图标、小字号元数据、未校准的效果图像素或工具损坏的比较结果固化为设计规则。
- **Don't** 将局部 UI 修正通过、组件计划批准或合成预览声明为全表面、首屏、真实设备或模型效果验收。


### Target and Settings Forms

设备与浏览器目标使用带标签的原生表单，保存时锁定字段，失败保留输入并就地显示原因。屏幕模式使用原生单选控件，帧率在镜像与虚拟屏均可操作。目标名称和网址优先展示；账号资料、尺寸、配置标识和版本按需展开。

配置标识独立分配，“从当前目标添加”只补充名称与适用目标。复制窗口只允许编辑复制接口接收的标识，原名称、版本与目标保留，复制后通过详情修改。设置按日常与高级用途分组；重新读取修改中的设置须明确确认，保存按钮反映实际修改状态。

常驻配置包在未配置目标时显示“选择配置包…”，名称与操作保持一行；切换仍等待现有草稿守卫。通用插件设置先验证真实表单描述，模型等自有编辑页面保留独立入口。窄屏设置先展示完整说明，再在同一行展示数值和生效时机。
