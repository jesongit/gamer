//! 单进程桌面启动器：主线程窗口/托盘，工作线程安装与持有服务端句柄。
use crate::{
    distribution as dist,
    layout::InstallLayout,
    manifest::model::Manifest,
    state::{lock::InstanceLock, StateStore},
    transfer::TransferControl,
};
use eframe::egui::{self, Color32, RichText, ViewportCommand};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Stdio},
    sync::{mpsc, Arc},
    time::Duration,
};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    TrayIcon, TrayIconBuilder, TrayIconEvent,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Checking,
    Install,
    Ready,
    Update,
    Working,
    Paused,
    Plugins,
    Running,
    Error,
    Stopped,
}
#[derive(Clone)]
struct Snapshot {
    stage: Stage,
    message: String,
    current: Option<String>,
    target: Option<String>,
    plugins: Vec<crate::official_plugins::Choice>,
    busy: bool,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            stage: Stage::Checking,
            message: "正在检查本地文件与可用更新…".into(),
            current: None,
            target: None,
            plugins: Vec::new(),
            busy: true,
        }
    }
}
enum Job {
    Inspect(bool),
    Install,
    Plugins(Vec<String>),
    Start,
    #[cfg(test)]
    Repair,
    #[cfg(test)]
    Stop,
    Exit,
}
fn permission_label(permission: &str) -> &str {
    match permission {
        "ui.host" => "工作台界面",
        "device.read" => "读取设备",
        "device.app" => "管理设备应用",
        "input.tap" => "点击",
        "input.swipe" => "滑动",
        "input.key" => "按键",
        "input.text" => "输入文字",
        "touch" => "触控",
        "vision.match" => "图像匹配",
        "vision.color" => "颜色识别",
        "resource.read" => "读取配置资源",
        "runtime.sleep" => "定时等待",
        "log.write" => "记录日志",
        "media.read" => "读取素材",
        "media.import" => "导入素材",
        "media.record" => "录制",
        "media.write" => "管理素材",
        "media.events.read" => "读取录制操作",
        other => other,
    }
}
enum Event {
    State(Snapshot),
    Hide,
    Exit,
}

pub fn run(layout: InstallLayout) -> i32 {
    let lock = match InstanceLock::acquire(&layout.state_dir()) {
        Ok(lock) => lock,
        Err(crate::state::lock::LockError::Held { .. }) => {
            // 本地唤醒信号仅属于该安装目录，重复双击不创建第二个服务端。
            let _ = fs::write(layout.state_dir().join("show-launcher"), b"show");
            return 0;
        }
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Gamer 启动器")
            .with_inner_size([440.0, 250.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_visible(false),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Gamer",
        options,
        Box::new(move |cc| {
            configure(&cc.egui_ctx);
            let (jobs, receiver) = mpsc::channel();
            let (events, updates) = mpsc::channel();
            let control = TransferControl::default();
            let worker_layout = layout.clone();
            let worker_control = control.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::Builder::new()
                .name("launcher-worker".into())
                .spawn(move || {
                    let _lock = lock;
                    Worker::run(worker_layout, worker_control, receiver, events, ctx);
                })?;
            let tray = make_tray(&cc.egui_ctx).ok();
            let quiet_start = tray.is_some();
            cc.egui_ctx
                .send_viewport_cmd(ViewportCommand::Visible(!quiet_start));
            jobs.send(Job::Inspect(true))?;
            Ok(Box::new(Desktop {
                layout,
                jobs,
                updates,
                control,
                snapshot: Snapshot::default(),
                tray,
                exit: false,
                cancelling: false,
                quiet_start,
            }))
        }),
    );
    if let Err(e) = result {
        tracing::error!(%e, "启动器窗口无法打开");
        return 1;
    }
    0
}

fn configure(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut fonts = egui::FontDefinitions::default();
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    for font in ["msyh.ttc", "simhei.ttf"] {
        if let Ok(bytes) = fs::read(PathBuf::from(&windows).join("Fonts").join(font)) {
            fonts
                .font_data
                .insert("gamer-cjk".into(), egui::FontData::from_owned(bytes).into());
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "gamer-cjk".into());
            break;
        }
    }
    ctx.set_fonts(fonts);
    let mut style = egui::Style {
        visuals: egui::Visuals::dark(),
        ..Default::default()
    };
    style.visuals.panel_fill = Color32::from_rgb(23, 25, 26);
    style.visuals.window_fill = Color32::from_rgb(29, 32, 33);
    style.visuals.override_text_color = Some(Color32::from_rgb(237, 240, 238));
    style.visuals.selection.bg_fill = Color32::from_rgb(90, 80, 30);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(40, 43, 45);
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    ctx.set_style_of(egui::Theme::Dark, style);
}
fn make_tray(ctx: &egui::Context) -> Result<TrayIcon, Box<dyn std::error::Error>> {
    let menu = Menu::new();
    for (id, label) in [("open", "打开 Gamer"), ("exit", "退出 Gamer")] {
        menu.append(&MenuItem::with_id(id, label, true, None))?;
    }
    let mut rgba = vec![0; 32 * 32 * 4];
    for y in 0..32 {
        for x in 0..32 {
            let offset = (y * 32 + x) * 4;
            let yellow = (6..26).contains(&x)
                && (6..26).contains(&y)
                && (x < 10
                    || !(10..=21).contains(&y)
                    || (x > 21 && y > 15)
                    || (y > 14 && y < 19 && x > 16));
            rgba[offset..offset + 4].copy_from_slice(if yellow {
                &[228, 201, 86, 255]
            } else {
                &[23, 25, 26, 255]
            });
        }
    }
    let wake = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |_| wake.request_repaint()));
    // 回调与 receiver 互斥，因此转发到本地 channel，见 GUI_EVENTS。
    let wake = ctx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = GUI_EVENTS.get().map(|tx| tx.send(e.id.0));
        wake.request_repaint();
    }));
    let wake = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |e| {
        if matches!(e, TrayIconEvent::DoubleClick { .. }) {
            let _ = GUI_EVENTS.get().map(|tx| tx.send("open".into()));
        }
        wake.request_repaint();
    }));
    Ok(TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Gamer")
        .with_icon(tray_icon::Icon::from_rgba(rgba, 32, 32)?)
        .build()?)
}
static GUI_EVENTS: std::sync::OnceLock<mpsc::Sender<String>> = std::sync::OnceLock::new();

#[cfg(test)]
#[path = "desktop_tests.rs"]
mod tests;
thread_local! { static GUI_RECEIVER: std::cell::RefCell<Option<mpsc::Receiver<String>>> = const { std::cell::RefCell::new(None) }; }

struct Desktop {
    layout: InstallLayout,
    jobs: mpsc::Sender<Job>,
    updates: mpsc::Receiver<Event>,
    control: TransferControl,
    snapshot: Snapshot,
    tray: Option<TrayIcon>,
    exit: bool,
    cancelling: bool,
    quiet_start: bool,
}
impl Desktop {
    fn send(&mut self, job: Job) {
        if self.jobs.send(job).is_ok() {
            self.snapshot.busy = true;
        }
    }
    fn show(&mut self, ctx: &egui::Context) {
        self.quiet_start = false;
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }
    fn cancel(&mut self) {
        if !self.cancelling {
            self.cancelling = true;
            self.control.pause();
            self.send(Job::Exit);
        }
    }
    fn action(&mut self, action: &str, ctx: &egui::Context) {
        match action {
            "open" if self.snapshot.stage == Stage::Running && !self.snapshot.busy => {
                crate::commands::open_browser(crate::supervisor::read_configured_port(
                    &self.layout.config_file(),
                ))
            }
            "open" => self.show(ctx),
            "exit" => self.cancel(),
            _ => {}
        }
    }
}
impl eframe::App for Desktop {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        // eframe may show its first rendered frame regardless of the initial builder flag.
        // Keep startup hidden until an explicit interaction or actionable state reveals it.
        if self.quiet_start {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }
        if GUI_EVENTS.get().is_none() {
            let (tx, rx) = mpsc::channel();
            let _ = GUI_EVENTS.set(tx);
            GUI_RECEIVER.with(|slot| *slot.borrow_mut() = Some(rx));
        }
        GUI_RECEIVER.with(|slot| {
            if let Some(rx) = slot.borrow().as_ref() {
                while let Ok(action) = rx.try_recv() {
                    self.action(&action, ctx);
                }
            }
        });
        while let Ok(event) = self.updates.try_recv() {
            match event {
                Event::State(state) => {
                    if self.cancelling && state.stage == Stage::Error {
                        self.cancelling = false;
                        self.control.resume();
                    }
                    if !self.cancelling
                        && matches!(
                            state.stage,
                            Stage::Install
                                | Stage::Update
                                | Stage::Plugins
                                | Stage::Paused
                                | Stage::Error
                        )
                    {
                        self.show(ctx);
                    }
                    let height = match state.stage {
                        Stage::Plugins => 320.0,
                        Stage::Error => 300.0,
                        _ => 250.0,
                    };
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(440.0, height)));
                    self.snapshot = state;
                }
                Event::Hide if self.tray.is_some() => {
                    ctx.send_viewport_cmd(ViewportCommand::Visible(false))
                }
                Event::Hide => {}
                Event::Exit => {
                    self.exit = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
        let show = self.layout.state_dir().join("show-launcher");
        if show.exists() {
            let _ = fs::remove_file(show);
            self.action("open", ctx);
        }
        if !self.exit && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            if self.tray.is_some() {
                ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            } else {
                self.cancel();
            }
        }
        if ctx.input(|i| i.viewport().minimized == Some(true)) && self.tray.is_some() {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(23, 25, 26))
                    .inner_margin(20),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("GAMER")
                            .size(22.0)
                            .strong()
                            .color(Color32::from_rgb(228, 201, 86)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let version = match (&self.snapshot.current, &self.snapshot.target) {
                            (Some(current), Some(target))
                                if self.snapshot.stage == Stage::Update =>
                            {
                                format!("{current} → {target}")
                            }
                            (_, Some(target)) => format!("v{target}"),
                            (Some(current), _) => format!("v{current}"),
                            _ => String::new(),
                        };
                        ui.label(RichText::new(version).small().weak());
                    });
                });
                ui.add_space(6.0);
                // Keep the two actions visible even when permissions or errors need scrolling.
                let body_height = (ui.available_height() - 48.0).max(40.0);
                egui::ScrollArea::vertical()
                    .id_salt("launcher-body")
                    .max_height(body_height)
                    .min_scrolled_height(body_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let title = if self.cancelling {
                            "正在安全取消…"
                        } else {
                            match self.snapshot.stage {
                                Stage::Checking => "正在启动…",
                                Stage::Install => "安装 Gamer",
                                Stage::Update => "有新版本可用",
                                Stage::Plugins => "选择需要的功能",
                                Stage::Working => "正在准备…",
                                Stage::Paused => "已暂停",
                                Stage::Error => "暂时无法继续",
                                Stage::Running => "Gamer 正在运行",
                                Stage::Ready | Stage::Stopped => "准备就绪",
                            }
                        };
                        ui.label(RichText::new(title).size(17.0).strong());
                        if self.cancelling {
                            ui.label("保留已下载内容，安全结束后退出。");
                        } else {
                            match self.snapshot.stage {
                                Stage::Install => {
                                    ui.label("自动准备软件和所需组件。");
                                    let root = self.layout.root.display().to_string();
                                    let root = root.trim_start_matches(r"\\?\");
                                    ui.add(
                                        egui::Label::new(RichText::new(root).small().weak())
                                            .truncate(),
                                    )
                                    .on_hover_text(root);
                                }
                                Stage::Update => {
                                    ui.label("更新完成后自动打开工作台。");
                                }
                                Stage::Plugins => {
                                    ui.label(
                                        RichText::new("可选，之后也能在工作台安装。")
                                            .small()
                                            .weak(),
                                    );
                                    for plugin in &mut self.snapshot.plugins {
                                        egui::collapsing_header::CollapsingState::load_with_default_open(
                                            ui.ctx(), ui.make_persistent_id(&plugin.id), false,
                                        )
                                            .show_header(ui, |ui| {
                                                ui.checkbox(&mut plugin.selected, &plugin.name);
                                                ui.label(RichText::new("权限").small().weak());
                                            })
                                            .body(|ui| {
                                                ui.label(
                                                    RichText::new(
                                                        plugin
                                                            .permissions
                                                            .iter()
                                                            .map(|p| permission_label(p))
                                                            .collect::<Vec<_>>()
                                                            .join("、"),
                                                    )
                                                    .small()
                                                    .weak(),
                                                )
                                                .on_hover_text(plugin.permissions.join("、"));
                                            });
                                    }
                                    ui.label(
                                        RichText::new("继续即同意所选插件的权限。").small().weak(),
                                    );
                                }
                                Stage::Running => {
                                    ui.label("关闭窗口后继续在托盘运行。");
                                }
                                Stage::Ready | Stage::Stopped => {
                                    ui.label("点击启动即可打开工作台。");
                                }
                                _ => {
                                    ui.label(&self.snapshot.message);
                                }
                            }
                        }
                        let (done, total) = self.control.progress();
                        if self.snapshot.busy && !self.cancelling {
                            ui.add_space(6.0);
                            if total > 0 {
                                ui.add(egui::ProgressBar::new(done as f32 / total as f32).text(
                                    format!(
                                        "{:.1} / {:.1} MB",
                                        done as f64 / 1048576.0,
                                        total as f64 / 1048576.0
                                    ),
                                ));
                            } else {
                                ui.spinner();
                            }
                        }
                    });
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let primary = if self.snapshot.busy {
                        "暂停"
                    } else {
                        match self.snapshot.stage {
                            Stage::Install => "安装",
                            Stage::Update => "更新",
                            Stage::Plugins | Stage::Paused => "继续",
                            Stage::Running => "打开网页",
                            Stage::Error => "重试",
                            _ => "启动",
                        }
                    };
                    let can_pause = self.snapshot.busy && self.snapshot.stage == Stage::Working;
                    if ui
                        .add_enabled(
                            !self.cancelling && (!self.snapshot.busy || can_pause),
                            egui::Button::new(
                                RichText::new(primary).color(Color32::from_rgb(23, 25, 26)),
                            )
                            .min_size(egui::vec2(88.0, 32.0))
                            .fill(Color32::from_rgb(228, 201, 86)),
                        )
                        .clicked()
                    {
                        if self.snapshot.busy {
                            self.control.pause();
                        } else {
                            self.control.resume();
                            match self.snapshot.stage {
                                Stage::Install | Stage::Update | Stage::Paused => {
                                    self.send(Job::Install)
                                }
                                Stage::Plugins => self.send(Job::Plugins(
                                    self.snapshot
                                        .plugins
                                        .iter()
                                        .filter(|p| p.selected)
                                        .map(|p| p.id.clone())
                                        .collect(),
                                )),
                                Stage::Running => self.action("open", ui.ctx()),
                                Stage::Error => self.send(Job::Inspect(false)),
                                _ => self.send(Job::Start),
                            }
                        }
                    }
                    let secondary = match self.snapshot.stage {
                        Stage::Plugins => "跳过",
                        Stage::Running => "收起",
                        _ => "取消",
                    };
                    if ui
                        .add_enabled(
                            !self.cancelling,
                            egui::Button::new(secondary).min_size(egui::vec2(88.0, 32.0)),
                        )
                        .clicked()
                    {
                        match self.snapshot.stage {
                            Stage::Plugins if !self.snapshot.busy => {
                                self.send(Job::Plugins(vec![]))
                            }
                            Stage::Running | Stage::Update
                                if self.tray.is_some() && !self.snapshot.busy =>
                            {
                                ui.ctx().send_viewport_cmd(ViewportCommand::Visible(false));
                            }
                            _ => self.cancel(),
                        }
                    }
                });
            });
    }
}

struct Worker {
    layout: InstallLayout,
    control: TransferControl,
    events: mpsc::Sender<Event>,
    ctx: egui::Context,
    state: Snapshot,
    candidate: Option<PathBuf>,
    child: Option<Child>,
    engine: Arc<crate::upgrade::engine::Engine>,
    dispatcher: Arc<crate::ipc::Dispatcher>,
    extras: crate::supervisor::LaunchExtras,
}
impl Worker {
    fn run(
        layout: InstallLayout,
        control: TransferControl,
        jobs: mpsc::Receiver<Job>,
        events: mpsc::Sender<Event>,
        ctx: egui::Context,
    ) {
        let store = StateStore::new(&layout.root);
        let setup = (|| -> Result<_, String> {
            let id = crate::installation::load_or_create(&store).map_err(|e| e.to_string())?;
            let token = crate::installation::new_session_token().map_err(|e| e.to_string())?;
            let admin = crate::installation::load_or_create_admin_token(&store)
                .map_err(|e| e.to_string())?;
            let pipe = crate::installation::pipe_name_for(&id);
            let extras = crate::supervisor::LaunchExtras::managed(pipe.clone(), token.clone())
                .with_admin_token(Some(admin.clone()));
            let options = crate::upgrade::engine::UpgradeOptions {
                fetch: crate::fetch::FetchOptions {
                    control: control.clone(),
                    ..Default::default()
                },
                admin_token: Some(admin),
                ipc: Some((pipe.clone(), token.clone())),
                ..Default::default()
            };
            let source = crate::upgrade::engine::ManifestSource::configured();
            let dispatcher =
                crate::ipc::Dispatcher::new(layout.clone(), id, source, options, false);
            let engine = dispatcher.engine.clone();
            let ipc_dispatcher = dispatcher.clone();
            std::thread::spawn(move || {
                if let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    let _ = rt.block_on(crate::ipc::run_server(
                        ipc_dispatcher,
                        crate::ipc::IpcServerConfig {
                            pipe_name: pipe,
                            token,
                            ..Default::default()
                        },
                    ));
                }
            });
            Ok((engine, dispatcher, extras))
        })();
        let (engine, dispatcher, extras) = match setup {
            Ok(v) => v,
            Err(e) => {
                let _ = events.send(Event::State(Snapshot {
                    stage: Stage::Error,
                    message: e,
                    busy: false,
                    ..Default::default()
                }));
                ctx.request_repaint();
                while let Ok(job) = jobs.recv() {
                    if matches!(job, Job::Exit) {
                        let _ = events.send(Event::Exit);
                        ctx.request_repaint();
                        return;
                    }
                    if matches!(job, Job::Inspect(_)) {
                        Self::run(layout, control, jobs, events, ctx);
                        return;
                    }
                }
                return;
            }
        };
        let mut worker = Self {
            layout,
            control,
            events,
            ctx,
            state: Snapshot::default(),
            candidate: None,
            child: None,
            engine,
            dispatcher,
            extras,
        };
        loop {
            if let Some(child) = worker.engine.take_managed_child() {
                worker.child = Some(child);
            }
            if worker
                .child
                .as_mut()
                .is_some_and(|c| c.try_wait().ok().flatten().is_some())
            {
                worker.child = None;
                worker.state.stage = Stage::Stopped;
                worker.state.message = "Gamer 已停止".into();
                worker.publish();
            }
            match jobs.recv_timeout(Duration::from_millis(500)) {
                Ok(job) => {
                    if worker.process(job) {
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => {
                    let _ = worker.stop();
                    return;
                }
            }
        }
    }

    fn process(&mut self, job: Job) -> bool {
        let _operation = match self.dispatcher.begin_desktop() {
            Ok(guard) => guard,
            Err(e) => {
                self.state.message = e;
                self.publish();
                return false;
            }
        };
        let exiting = matches!(job, Job::Exit);
        self.state.busy = true;
        self.state.stage = Stage::Working;
        self.publish();
        let result = match job {
            Job::Inspect(auto) => self.inspect(auto),
            Job::Install => self.install(),
            Job::Plugins(selected) => self.install_plugins(selected),
            Job::Start => self.start(),
            #[cfg(test)]
            Job::Repair => self.repair(),
            #[cfg(test)]
            Job::Stop => self.stop(),
            Job::Exit => self.stop(),
        };
        self.state.busy = false;
        if let Err(e) = result {
            self.state.stage = if self.control.is_paused() {
                Stage::Paused
            } else {
                Stage::Error
            };
            self.state.message = e;
        } else if exiting {
            let _ = self.events.send(Event::Exit);
            self.ctx.request_repaint();
            return true;
        }
        self.publish();
        false
    }
    fn publish(&self) {
        let _ = self.events.send(Event::State(self.state.clone()));
        self.ctx.request_repaint();
    }
    fn inspect(&mut self, auto: bool) -> Result<(), String> {
        if auto {
            let report = crate::upgrade::recovery::recover_on_startup(
                &self.layout,
                &StateStore::new(&self.layout.root),
            )
            .map_err(|e| e.to_string())?;
            if report.is_manual() {
                return Err("上次更新需要人工恢复，请查看 logs/launcher.log".into());
            }
        }
        self.state.current = dist::current(&self.layout);
        let remote = dist::discover(&self.layout);
        let network_message = remote
            .as_ref()
            .err()
            .map(|e| format!("暂时无法联网，使用已验证的本地发行信息：{e}"));
        let cached = dist::cached(&self.layout, None);
        let candidate = match (remote.ok(), cached) {
            (Some(a), Some(b)) => Some(
                if dist::compare_versions(&a.1.release.version, &b.1.release.version).is_lt() {
                    b
                } else {
                    a
                },
            ),
            (a, b) => a.or(b),
        };
        let Some((path, model)) = candidate else {
            return Err(network_message
                .unwrap_or_else(|| "未找到发行清单，请联网重试或解压完整离线包到此目录".into()));
        };
        self.candidate = Some(path);
        self.state.target = Some(model.release.version.clone());
        let platform = model
            .platforms
            .get(dist::PLATFORM)
            .ok_or("发行平台不匹配")?;
        if self.state.current.is_none() {
            self.state.stage = Stage::Install;
            self.state.message = "软件与所需组件将安装到启动器所在目录".into();
            return Ok(());
        }
        let current = self.state.current.clone().unwrap();
        if dist::compare_versions(&model.release.version, &current).is_gt()
            || platform.components.iter().any(|c| {
                c.id == "launcher"
                    && dist::compare_versions(&c.version, env!("CARGO_PKG_VERSION")).is_gt()
            })
        {
            self.state.stage = Stage::Update;
            self.state.message = "更新已准备好，安装前不会自动启动软件".into();
            return Ok(());
        }
        self.state.stage = if self.child.is_some() {
            Stage::Running
        } else {
            Stage::Ready
        };
        self.state.message = network_message.unwrap_or_else(|| "当前已是最新版本".into());
        if auto {
            self.start()?;
        }
        Ok(())
    }
    fn manifest(&self, current: bool) -> Result<(PathBuf, Manifest), String> {
        if current {
            let version = dist::current(&self.layout).ok_or("尚未安装")?;
            return dist::cached(&self.layout, Some(&version))
                .ok_or("当前版本的发行清单不可用".into());
        }
        let path = self.candidate.clone().ok_or("请先检查更新")?;
        let model = dist::read(&self.layout, &path)?;
        Ok((path, model))
    }
    fn install(&mut self) -> Result<(), String> {
        let (path, model) = self.manifest(false)?;
        if dist::current(&self.layout).is_none() {
            let platform = &model.platforms[dist::PLATFORM];
            // 首装先预估下载与展开空间；ZIP 安全解压再按真实声明字节检查。
            let mut needed = (platform.app.artifact.size as u64).saturating_mul(5);
            for component in &platform.components {
                needed = needed.saturating_add(component.artifact.size as u64);
                for file in &component.required_files {
                    needed = needed.saturating_add(file.size as u64);
                }
            }
            needed = needed.saturating_add(64 * 1024 * 1024);
            let available =
                crate::winutil::free_disk_bytes(&self.layout.root).map_err(|e| e.to_string())?;
            if available < needed {
                return Err(format!(
                    "磁盘空间不足：预计需要 {:.0} MB，可用 {:.0} MB",
                    needed as f64 / 1048576.0,
                    available as f64 / 1048576.0
                ));
            }
        }
        if let Some(component) = model.platforms.get(dist::PLATFORM).and_then(|p| {
            p.components.iter().find(|c| {
                c.id == "launcher"
                    && dist::compare_versions(&c.version, env!("CARGO_PKG_VERSION")).is_gt()
            })
        }) {
            self.stop()?;
            let spec = crate::inventory::ComponentSpec::from_model(component)?;
            self.state.message = "正在更新启动器，完成后自动重新打开…".into();
            self.publish();
            let report = crate::repair::repair_component(
                &self.layout,
                &spec,
                &crate::repair::RepairOptions {
                    fetch: crate::fetch::FetchOptions {
                        control: self.control.clone(),
                        ..Default::default()
                    },
                    probe: false,
                },
            );
            if let crate::repair::ComponentOutcome::Failed { reason } = report.outcome {
                return Err(reason);
            }
            let exe_spec = spec
                .files
                .iter()
                .find(|f| f.path == "gamer-launcher.exe")
                .ok_or("启动器组件缺少入口校验信息")?;
            crate::digest::verify_file(
                &spec.install_dir(&self.layout).join(&exe_spec.path),
                &exe_spec.sha256,
                exe_spec.size,
            )
            .map_err(|e| e.to_string())?;
            let mut request = crate::upgrade::trampoline::LauncherUpdateRequest::new(
                std::env::current_exe().map_err(|e| e.to_string())?,
                spec.install_dir(&self.layout).join("gamer-launcher.exe"),
            );
            request.restart = true;
            crate::upgrade::trampoline::schedule(&request).map_err(|e| e.to_string())?;
            let _ = self.events.send(Event::Exit);
            self.ctx.request_repaint();
            return Ok(());
        }
        if dist::compare_versions(
            env!("CARGO_PKG_VERSION"),
            &model.release.minimum_launcher_version,
        )
        .is_lt()
        {
            return Err(format!(
                "此版本需要启动器 {} 或更新版本",
                model.release.minimum_launcher_version
            ));
        }
        if dist::current(&self.layout).is_some_and(|v| v != model.release.version) {
            self.state.message = "正在下载并更新，数据切换前将创建恢复快照".into();
            self.publish();
            match self
                .engine
                .run_full(&crate::upgrade::engine::ManifestSource::Path(path))
            {
                crate::upgrade::engine::UpgradeOutcome::Committed { .. } => {
                    self.child = self.engine.take_managed_child();
                    self.state.current = Some(model.release.version.clone());
                    self.state.stage = Stage::Running;
                    self.state.message = "更新完成".into();
                    return self.first_plugins(&model);
                }
                other => return Err(format!("更新未完成：{other:?}")),
            }
        }
        self.repair_model(&model, false)?;
        self.start()
    }
    fn repair_model(&mut self, model: &Manifest, deep: bool) -> Result<(), String> {
        let platform = model
            .platforms
            .get(dist::PLATFORM)
            .ok_or("发行平台不匹配")?;
        let specs = platform
            .components
            .iter()
            .map(crate::inventory::ComponentSpec::from_model)
            .collect::<Result<Vec<_>, _>>()?;
        let app = crate::repair::AppInstallSpec::from_model(platform, &model.release.version)?;
        self.state.message = "校验文件并安装缺失内容…".into();
        self.publish();
        let report = crate::repair::repair_components(
            &self.layout,
            &specs,
            Some(&app),
            &crate::repair::RepairOptions {
                fetch: crate::fetch::FetchOptions {
                    control: self.control.clone(),
                    ..Default::default()
                },
                probe: deep,
            },
        );
        if report.failed_count() > 0 {
            let mut failures: Vec<String> = report
                .components
                .iter()
                .filter_map(|c| match &c.outcome {
                    crate::repair::ComponentOutcome::Failed { reason } => {
                        Some(format!("{}：{reason}", c.id))
                    }
                    _ => None,
                })
                .collect();
            if let Some(crate::repair::AppRepair {
                outcome: crate::repair::AppOutcome::Failed { reason },
                ..
            }) = report.app
            {
                failures.push(reason);
            }
            return Err(failures.join("\n"));
        }
        Ok(())
    }
    #[cfg(test)]
    fn repair(&mut self) -> Result<(), String> {
        self.stop()?;
        let (_, model) = self.manifest(true)?;
        self.repair_model(&model, true)?;
        self.state.stage = Stage::Ready;
        self.state.message = "文件校验完成，可以启动".into();
        Ok(())
    }
    fn start(&mut self) -> Result<(), String> {
        let (_, model) = self.manifest(true)?;
        if self.child.is_some() {
            crate::supervisor::wait_for_ready(
                crate::supervisor::read_configured_port(&self.layout.config_file()),
                &Default::default(),
            )?;
            return self.first_plugins(&model);
        }
        self.repair_model(&model, false)?;
        fs::create_dir_all(self.layout.root.join("config")).map_err(|e| e.to_string())?;
        if !self.layout.config_file().exists() {
            fs::write(self.layout.config_file(), "port = 8443\n").map_err(|e| e.to_string())?;
        }
        fs::create_dir_all(self.layout.data_dir()).map_err(|e| e.to_string())?;
        let plan = dist::plan(&self.layout, &model)?;
        let port = crate::supervisor::read_configured_port(&plan.config_path);
        if std::net::TcpStream::connect_timeout(
            &([127, 0, 0, 1], port).into(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            return Err(format!(
                "端口 {port} 已被占用，请停止占用它的程序或修改 config/config.toml"
            ));
        }
        self.child = Some(
            crate::supervisor::spawn_child_with_extras(
                &plan,
                &[],
                &self.extras,
                Stdio::null(),
                Stdio::null(),
            )
            .map_err(|e| e.to_string())?,
        );
        crate::supervisor::wait_for_ready(port, &Default::default())?;
        self.state.current = Some(model.release.version.clone());
        self.state.stage = Stage::Running;
        self.state.message = "已启动，关闭窗口后继续在托盘运行".into();
        self.first_plugins(&model)
    }
    fn first_plugins(&mut self, model: &Manifest) -> Result<(), String> {
        if self.control.is_paused() {
            return Err("操作已暂停，已完成的内容会保留".into());
        }
        if !self.layout.state_dir().join("plugins-choice.json").exists() {
            self.state.plugins = crate::official_plugins::choices(&self.layout, model)?;
            if !self.state.plugins.is_empty() {
                self.state.stage = Stage::Plugins;
                self.state.message = "软件已安装，选择需要的功能插件".into();
                return Ok(());
            }
        }
        self.state.stage = Stage::Running;
        crate::commands::open_browser(crate::supervisor::read_configured_port(
            &self.layout.config_file(),
        ));
        let _ = self.events.send(Event::Hide);
        Ok(())
    }
    fn install_plugins(&mut self, selected: Vec<String>) -> Result<(), String> {
        self.state.message = "正在安装所选插件…".into();
        self.publish();
        let token = self.extras.admin_token.as_deref().ok_or("管理凭据不可用")?;
        crate::official_plugins::install(&self.layout, &self.state.plugins, &selected, token)?;
        self.state.plugins.clear();
        let (_, model) = self.manifest(true)?;
        self.first_plugins(&model)
    }
    fn stop(&mut self) -> Result<(), String> {
        if let Some(child) = self.engine.take_managed_child() {
            self.child = Some(child);
        }
        if let Some(child) = self.child.as_mut() {
            if child.try_wait().map_err(|e| e.to_string())?.is_none() {
                let port = crate::supervisor::read_configured_port(&self.layout.config_file());
                let token = self.extras.admin_token.as_deref().ok_or("管理凭据不可用")?;
                self.state.message = "正在停止任务与设备会话…".into();
                self.publish();
                let response = crate::upgrade::httpc::http_request(
                    ([127, 0, 0, 1], port).into(),
                    "POST",
                    "/api/shutdown",
                    &[("X-Admin-Token", token)],
                    Duration::from_secs(95),
                )?;
                if response.status != 200 && response.status != 202 {
                    return Err(format!("停止请求被拒绝（{}）", response.status));
                }
                let deadline = std::time::Instant::now() + Duration::from_secs(15);
                while self
                    .child
                    .as_mut()
                    .unwrap()
                    .try_wait()
                    .map_err(|e| e.to_string())?
                    .is_none()
                {
                    if std::time::Instant::now() > deadline {
                        return Err("服务仍在退出，启动器将继续监管".into());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        self.child = None;
        self.state.stage = Stage::Stopped;
        self.state.message = "Gamer 已停止".into();
        Ok(())
    }
}
