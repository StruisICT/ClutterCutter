// Cross-platform egui frontend for ClutterCutter (Linux / macOS / Windows).
//
// The native Win32 GUI (the `cluttercutter` binary) stays the premium Windows
// build; this eframe/egui app is the portable one. It shares the scan core from
// the library crate (walk / analysis / types / drives / format), so only the
// presentation differs. Phase 2: window + theme + drive cards + top-level list.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod palette;

use cluttercutter::drives::{self, DriveInfo};
use cluttercutter::format::format_bytes;
use cluttercutter::types::{FolderNode, ScanProgress};
use cluttercutter::walk::WalkScanner;
use eframe::egui::{self, CornerRadius, FontId, Sense, Stroke};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::thread;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("ClutterCutter"),
        ..Default::default()
    };
    eframe::run_native(
        "ClutterCutter",
        native_options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

enum ScanMsg {
    Progress(ScanProgress),
    Done(Box<FolderNode>),
}

struct App {
    dark: bool,
    drives: Vec<DriveInfo>,
    selected: Option<usize>,
    scanning: bool,
    progress: Option<ScanProgress>,
    root: Option<FolderNode>,
    rx: Option<Receiver<ScanMsg>>,
    cancel: Arc<AtomicBool>,
    styled: bool,
    // Dev tooling: if CC_SHOT=<path> is set, save one framebuffer screenshot to
    // that path after a few frames and exit. glow renders to the GL surface, so
    // OS-level window capture (PrintWindow) can't grab it — this can.
    shot: Option<String>,
    frame: u32,
    // CC_SCAN=<path>: auto-scan a path on launch (dev/screenshot helper).
    auto_scan: Option<String>,
    shot_requested: bool,
}

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            dark: false,
            drives: drives::list_drives(),
            selected: None,
            scanning: false,
            progress: None,
            root: None,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            styled: false,
            shot: std::env::var("CC_SHOT").ok(),
            frame: 0,
            auto_scan: std::env::var("CC_SCAN").ok(),
            shot_requested: false,
        }
    }

    fn handle_shot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.shot.clone() else {
            return;
        };
        let img = ctx.input(|i| {
            i.events
                .iter()
                .chain(i.raw.events.iter())
                .find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
        });
        if let Some(img) = img {
            let [w, h] = img.size;
            let _ = image::save_buffer(
                &path,
                img.as_raw(),
                w as u32,
                h as u32,
                image::ExtendedColorType::Rgba8,
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if !self.shot_requested && !self.scanning && self.frame >= 4 {
            self.shot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        } else if self.frame > 300 {
            // Safety: never got the screenshot back — don't hang forever.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }

    fn start_scan(&mut self, idx: usize) {
        if let Some(drive) = self.drives.get(idx).cloned() {
            self.selected = Some(idx);
            let hint = drive.used() as i64;
            self.scan_path(drive.path, hint);
        }
    }

    fn scan_path(&mut self, root_path: String, hint: i64) {
        // Cancel any in-flight scan and start fresh.
        self.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        self.scanning = true;
        self.root = None;
        self.progress = Some(ScanProgress {
            percent: -1.0,
            ..Default::default()
        });

        let (tx, rx): (Sender<ScanMsg>, Receiver<ScanMsg>) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        thread::spawn(move || {
            let ptx = tx.clone();
            let node = WalkScanner::new()
                .with_cancel(cancel)
                .with_size_hint(hint)
                .with_progress(Box::new(move |p: &ScanProgress| {
                    let _ = ptx.send(ScanMsg::Progress(p.clone()));
                }))
                .scan(&root_path);
            if let Ok(node) = node {
                let _ = tx.send(ScanMsg::Done(Box::new(node)));
            }
        });
    }

    fn drain(&mut self, ctx: &egui::Context) {
        let mut done = false;
        if let Some(rx) = &self.rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    ScanMsg::Progress(p) => self.progress = Some(p),
                    ScanMsg::Done(node) => {
                        self.root = Some(*node);
                        done = true;
                    }
                }
            }
        }
        if done {
            self.scanning = false;
            self.progress = None;
            self.rx = None;
        }
        if self.scanning {
            ctx.request_repaint();
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame = self.frame.saturating_add(1);
        if self.frame == 2 {
            if let Some(p) = self.auto_scan.take() {
                self.scan_path(p, 0);
            }
        }
        if !self.styled {
            palette::apply(&ctx, self.dark);
            self.styled = true;
        }
        self.drain(&ctx);
        self.handle_shot(&ctx);
        let pal = palette::palette(self.dark);

        // ---- top bar ----
        egui::Panel::top("topbar")
            .exact_size(48.0)
            .frame(
                egui::Frame::new()
                    .fill(pal.panel_bg)
                    .inner_margin(egui::Margin::symmetric(14, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(
                        egui::RichText::new("ClutterCutter")
                            .color(pal.blue)
                            .strong()
                            .size(20.0),
                    );
                    ui.label(
                        egui::RichText::new("Struis ICT")
                            .color(pal.subtext)
                            .size(12.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let icon = if self.dark { "☀" } else { "🌙" };
                        if ui.button(icon).on_hover_text("Toggle theme").clicked() {
                            self.dark = !self.dark;
                            self.styled = false;
                        }
                    });
                });
            });

        // ---- drives sidebar ----
        let mut clicked: Option<usize> = None;
        let drives = self.drives.clone();
        let selected = self.selected;
        egui::Panel::left("drives")
            .exact_size(232.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(pal.win_bg)
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("DRIVES")
                        .color(pal.subtext)
                        .size(12.0)
                        .strong(),
                );
                ui.add_space(8.0);
                for (i, d) in drives.iter().enumerate() {
                    if drive_card(ui, d, selected == Some(i), &pal).clicked() {
                        clicked = Some(i);
                    }
                    ui.add_space(8.0);
                }
                if drives.is_empty() {
                    ui.label(egui::RichText::new("No drives found").color(pal.subtext));
                }
            });
        if let Some(i) = clicked {
            self.start_scan(i);
        }

        // ---- central content ----
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(pal.win_bg)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ui, |ui| {
                if self.scanning {
                    self.show_scanning(ui, &pal);
                } else if let Some(root) = &self.root {
                    show_listing(ui, root, &pal);
                } else {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Select a drive to scan")
                                .color(pal.subtext)
                                .size(16.0),
                        );
                    });
                }
            });
    }
}

impl App {
    fn show_scanning(&self, ui: &mut egui::Ui, pal: &palette::Pal) {
        let p = self.progress.clone().unwrap_or_default();
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Scanning…")
                    .color(pal.text)
                    .size(18.0)
                    .strong(),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "{} files · {}",
                    p.files_scanned,
                    format_bytes(p.total_size)
                ))
                .color(pal.subtext),
            );
            ui.add_space(12.0);
            let frac = if p.percent < 0.0 {
                None
            } else {
                Some((p.percent / 100.0) as f32)
            };
            let bar = egui::ProgressBar::new(frac.unwrap_or(0.0))
                .desired_width(360.0)
                .fill(pal.blue)
                .animate(frac.is_none());
            ui.add(bar);
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(shorten(&p.current_path, 64))
                    .color(pal.subtext)
                    .size(11.0),
            );
        });
    }
}

// A drive card: label, "used of total", and a usage bar. Whole card is clickable.
fn drive_card(
    ui: &mut egui::Ui,
    d: &DriveInfo,
    selected: bool,
    pal: &palette::Pal,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 66.0), Sense::click());
    let p = ui.painter();
    let bg = if selected {
        pal.card_sel
    } else if resp.hovered() {
        pal.card_bg.linear_multiply(0.97)
    } else {
        pal.card_bg
    };
    p.rect_filled(rect, CornerRadius::same(8), bg);
    if selected {
        p.rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.5, pal.blue),
            egui::StrokeKind::Inside,
        );
    } else {
        p.rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.0, pal.hairline),
            egui::StrokeKind::Inside,
        );
    }

    let pad = 10.0;
    p.text(
        rect.left_top() + egui::vec2(pad, 8.0),
        egui::Align2::LEFT_TOP,
        &d.label,
        FontId::proportional(15.0),
        pal.text,
    );
    p.text(
        rect.right_top() + egui::vec2(-pad, 9.0),
        egui::Align2::RIGHT_TOP,
        format!("{} free", format_bytes(d.free as i64)),
        FontId::proportional(11.0),
        pal.subtext,
    );

    // usage bar
    let bar = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(pad, 32.0),
        egui::vec2(width - 2.0 * pad, 8.0),
    );
    p.rect_filled(bar, CornerRadius::same(4), pal.track);
    let frac = d.used_fraction().clamp(0.0, 1.0);
    if frac > 0.0 {
        let fill = egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * frac, bar.height()));
        p.rect_filled(fill, CornerRadius::same(4), pal.blue);
    }
    p.text(
        rect.left_bottom() + egui::vec2(pad, -8.0),
        egui::Align2::LEFT_BOTTOM,
        format!(
            "{} of {}",
            format_bytes(d.used() as i64),
            format_bytes(d.total as i64)
        ),
        FontId::proportional(11.0),
        pal.subtext,
    );
    resp
}

// The scanned root's immediate children, largest first, each with a
// %-of-parent bar (green) and a size badge (blue).
fn show_listing(ui: &mut egui::Ui, root: &FolderNode, pal: &palette::Pal) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(&root.name)
                .color(pal.text)
                .size(16.0)
                .strong(),
        );
        ui.label(
            egui::RichText::new(format!(
                "· {} · {} files",
                format_bytes(root.size),
                root.file_count
            ))
            .color(pal.subtext),
        );
    });
    ui.add_space(10.0);

    let mut kids: Vec<&FolderNode> = root.children.iter().collect();
    kids.sort_by_key(|k| std::cmp::Reverse(k.size));
    let total = root.size.max(1) as f64;

    egui::ScrollArea::vertical().show(ui, |ui| {
        for k in kids {
            let frac = (k.size as f64 / total) as f32;
            list_row(ui, &k.name, frac, format_bytes(k.size), pal);
        }
    });
}

fn list_row(ui: &mut egui::Ui, name: &str, frac: f32, size: String, pal: &palette::Pal) {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 30.0), Sense::hover());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(5), pal.card_bg);
    }
    let pad = 8.0;
    // name (left)
    p.text(
        rect.left_center() + egui::vec2(pad, 0.0),
        egui::Align2::LEFT_CENTER,
        name,
        FontId::proportional(13.5),
        pal.text,
    );
    // size badge (right)
    p.text(
        rect.right_center() + egui::vec2(-pad, 0.0),
        egui::Align2::RIGHT_CENTER,
        size,
        FontId::proportional(13.0),
        pal.blue,
    );
    // %-of-parent bar in the middle band
    let bar_w = (width * 0.30).min(240.0);
    let bar = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 110.0 - bar_w, rect.center().y - 3.0),
        egui::vec2(bar_w, 6.0),
    );
    p.rect_filled(bar, CornerRadius::same(3), pal.track);
    if frac > 0.0 {
        let fill =
            egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * frac.clamp(0.0, 1.0), 6.0));
        p.rect_filled(fill, CornerRadius::same(3), pal.green);
    }
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let tail: String = s
        .chars()
        .rev()
        .take(max - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}
