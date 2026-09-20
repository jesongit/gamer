import { defineConfig } from 'vitepress'

const repository = process.env.GITHUB_REPOSITORY || 'jesongit/gamer'
const [owner, name] = repository.split('/')
const base = process.env.DOCS_BASE || (name === `${owner}.github.io` ? '/' : `/${name}/`)

export default defineConfig({
  lang: 'zh-CN',
  title: 'Gamer',
  titleTemplate: ':title · Gamer 文档',
  description: 'Gamer 官方文档：Android 投屏、可视化自动化、任务调度、按键映射、视频分析与插件开发。',
  base,
  srcDir: 'site',
  appearance: 'dark',
  cleanUrls: false,
  lastUpdated: true,
  head: [
    ['link', { rel: 'icon', type: 'image/svg+xml', href: `${base}logo.svg` }],
    ['meta', { name: 'theme-color', content: '#151619' }],
  ],
  markdown: { lineNumbers: true },
  themeConfig: {
    logo: '/logo.svg',
    siteTitle: 'Gamer 文档',
    nav: [
      { text: '开始使用', link: '/guide/getting-started', activeMatch: '/guide/' },
      { text: '插件开发', link: '/development/overview', activeMatch: '/development/' },
      { text: '参考', link: '/reference/YAML', activeMatch: '/reference/' },
    ],
    socialLinks: [{ icon: 'github', link: `https://github.com/${repository}` }],
    sidebar: [
      { text: '开始使用', items: [
        { text: '认识 Gamer', link: '/guide/getting-started' },
        { text: '安装与首次启动', link: '/guide/installation' },
        { text: '连接设备与虚拟屏', link: '/guide/devices' },
      ] },
      { text: '使用手册', items: [
        { text: '工作台与状态条', link: '/guide/workbench' },
        { text: '自动化与函数', link: '/guide/automation' },
        { text: '模板与图像匹配', link: '/guide/templates' },
        { text: '按键映射', link: '/guide/keymap' },
        { text: '视频与录制', link: '/guide/video' },
        { text: '任务与日志', link: '/guide/tasks' },
        { text: '配置包与插件', link: '/guide/packages' },
        { text: '常见问题', link: '/guide/troubleshooting' },
      ] },
      { text: '插件开发', items: [
        { text: '架构与开发环境', link: '/development/overview' },
        { text: '开发与分发插件', link: '/development/plugins' },
        { text: '界面与状态条接入', link: '/development/ui' },
        { text: '维护文档网站', link: '/development/documentation' },
      ] },
      { text: '参考资料', items: [
        { text: 'YAML 案例教程', link: '/guides/yaml-tutorial' },
        { text: 'YAML V1 语法', link: '/reference/YAML' },
        { text: '按键映射格式', link: '/reference/KEYMAP_SCHEMA' },
        { text: '服务端配置', link: '/reference/configuration' },
        { text: 'API 与 SDK 导航', link: '/reference/api' },
      ] },
    ],
    search: { provider: 'local', options: { locales: { root: { translations: {
      button: { buttonText: '搜索文档', buttonAriaLabel: '搜索文档' },
      modal: { noResultsText: '没有找到相关内容', resetButtonTitle: '清空搜索',
        footer: { selectText: '选择', navigateText: '切换', closeText: '关闭' } },
    } } } } },
    outline: { label: '本页目录', level: [2, 3] },
    docFooter: { prev: '上一篇', next: '下一篇' },
    editLink: {
      pattern: ({ relativePath }) => {
        const shared = ['reference/YAML.md', 'reference/KEYMAP_SCHEMA.md', 'guides/yaml-tutorial.md']
        // VitePress serializes this function for the browser; it must not capture config variables.
        return `https://github.com/jesongit/gamer/edit/main/docs/${shared.includes(relativePath) ? '' : 'site/'}${relativePath}`
      },
      text: '在 GitHub 上编辑此页',
    },
    lastUpdated: { text: '最后更新', formatOptions: { dateStyle: 'medium', timeStyle: 'short' } },
    darkModeSwitchLabel: '外观', lightModeSwitchTitle: '切换浅色模式', darkModeSwitchTitle: '切换深色模式',
    sidebarMenuLabel: '目录', returnToTopLabel: '返回顶部',
    notFound: { code: '404', title: '页面不存在', quote: '可以返回首页，或使用搜索找到需要的文档。', linkLabel: '返回文档首页', linkText: '返回文档首页' },
    footer: { message: 'Gamer · Android 游戏自动化工具', copyright: '文档随项目源码维护' },
  },
})
