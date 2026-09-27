//! Native hidden-window regression probe; never starts Gamer services or devices.
use eframe::egui;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

struct Probe {
    visible: Box<dyn Fn() -> Option<bool>>,
    was_visible: Arc<AtomicBool>,
    painted: Arc<AtomicUsize>,
    ticks: usize,
}
impl eframe::App for Probe {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if (self.visible)() == Some(true) {
            self.was_visible.store(true, Ordering::SeqCst);
        }
        self.ticks += 1;
        if self.ticks >= 10 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(40));
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.painted.fetch_add(1, Ordering::SeqCst);
        ui.label("Hidden startup probe");
    }
}
fn main() -> eframe::Result {
    let was_visible = Arc::new(AtomicBool::new(false));
    let painted = Arc::new(AtomicUsize::new(0));
    let observed = was_visible.clone();
    let rendered = painted.clone();
    eframe::run_native(
        "Gamer hidden startup probe",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_visible(false),
            ..Default::default()
        },
        Box::new(move |cc| {
            let window = cc.winit_window().cloned().expect("native window");
            Ok(Box::new(Probe {
                visible: Box::new(move || window.is_visible()),
                was_visible: observed,
                painted: rendered,
                ticks: 0,
            }))
        }),
    )?;
    assert!(
        painted.load(Ordering::SeqCst) > 0,
        "must exercise first paint"
    );
    assert!(
        !was_visible.load(Ordering::SeqCst),
        "hidden window became visible after first paint"
    );
    println!(
        "PASS: {} UI frames; native window remained hidden",
        painted.load(Ordering::SeqCst)
    );
    Ok(())
}
