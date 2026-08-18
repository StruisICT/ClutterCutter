// Cross-platform egui frontend for ClutterCutter (Linux / macOS / Windows).
//
// The native Win32 GUI (the `cluttercutter` binary) stays the premium Windows
// build; this eframe/egui app is the portable one. It shares the scan core from
// the library crate (walk / analysis / types / drives / format), so only the
// presentation differs. Phase 2: window + theme + drive cards + top-level list.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod palette;

use cluttercutter::drives::{self, DriveInfo};
use cluttercutter::format::{format_bytes, set_binary_units};
use cluttercutter::types::{FileEntry, FolderNode, ScanProgress};
use cluttercutter::walk::WalkScanner;
use cluttercutter::{analysis, datetime, tempscan};
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

#[derive(Clone, Copy, PartialEq)]
enum View {
    Browse,
    Largest,
    Oldest,
}

// A navigation intent recorded during rendering, applied after the borrow of
// the scan tree is released.
enum NavAction {
    Drill(usize),
    Up,
    Back,
    Fwd,
    Crumb(usize),
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
    // Navigation into the scanned tree: `cur` is the child-index path from root;
    // back/fwd are history stacks.
    cur: Vec<usize>,
    back: Vec<Vec<usize>>,
    fwd: Vec<Vec<usize>>,
    view: View,
    search: String,
    // CC_SEARCH=<query>: apply an initial search once the auto-scan lands (dev).
    pending_search: Option<String>,
    // Extra scan sources + settings.
    temp_roots: Vec<tempscan::TempRoot>,
    path_input: String,
    binary_units: bool,
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
            cur: Vec::new(),
            back: Vec::new(),
            fwd: Vec::new(),
            view: View::Browse,
            search: String::new(),
            pending_search: std::env::var("CC_SEARCH").ok(),
            temp_roots: tempscan::temp_locations(),
            path_input: String::new(),
            binary_units: true,
        }
    }

    fn apply_nav(&mut self, a: Option<NavAction>) {
        let Some(a) = a else {
            return;
        };
        match a {
            NavAction::Drill(i) => {
                self.back.push(self.cur.clone());
                self.fwd.clear();
                self.cur.push(i);
            }
            NavAction::Up => {
                if !self.cur.is_empty() {
                    self.back.push(self.cur.clone());
                    self.fwd.clear();
                    self.cur.pop();
                }
            }
            NavAction::Back => {
                if let Some(p) = self.back.pop() {
                    self.fwd.push(std::mem::replace(&mut self.cur, p));
                }
            }
            NavAction::Fwd => {
                if let Some(p) = self.fwd.pop() {
                    self.back.push(std::mem::replace(&mut self.cur, p));
                }
            }
            NavAction::Crumb(k) => {
                if k < self.cur.len() {
                    self.back.push(self.cur.clone());
                    self.fwd.clear();
                    self.cur.truncate(k);
                }
            }
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
                .with_track_files(true)
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
                        // Reset navigation/view/search for the new tree.
                        self.cur.clear();
                        self.back.clear();
                        self.fwd.clear();
                        self.view = View::Browse;
                        self.search = self.pending_search.take().unwrap_or_default();
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
        // A scan requested this frame (folder box or a temp/cache entry), applied
        // after the panels so we never re-borrow self mid-closure.
        let mut scan_request: Option<String> = None;

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
                        let units = if self.binary_units { "1024" } else { "1000" };
                        if ui
                            .button(units)
                            .on_hover_text("Toggle binary (1024) / decimal (1000) units")
                            .clicked()
                        {
                            self.binary_units = !self.binary_units;
                            set_binary_units(self.binary_units);
                        }
                        if ui.button("Scan folder").clicked() {
                            let p = self.path_input.trim().to_string();
                            if !p.is_empty() {
                                scan_request = Some(p);
                            }
                        }
                        let te = ui.add(
                            egui::TextEdit::singleline(&mut self.path_input)
                                .hint_text("Folder path…")
                                .desired_width(220.0),
                        );
                        if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            let p = self.path_input.trim().to_string();
                            if !p.is_empty() {
                                scan_request = Some(p);
                            }
                        }
                    });
                });
            });

        // ---- drives sidebar ----
        let mut clicked: Option<usize> = None;
        let drives = self.drives.clone();
        let selected = self.selected;
        let temp_roots: Vec<(String, String)> = self
            .temp_roots
            .iter()
            .map(|t| (t.label.clone(), t.path.clone()))
            .collect();
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
                if !temp_roots.is_empty() {
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new("TEMP / CACHES")
                            .color(pal.subtext)
                            .size(12.0)
                            .strong(),
                    );
                    ui.add_space(6.0);
                    let w = ui.available_width();
                    for (label, path) in &temp_roots {
                        if ui
                            .add_sized([w, 26.0], egui::Button::new(label))
                            .on_hover_text(path.as_str())
                            .clicked()
                        {
                            scan_request = Some(path.clone());
                        }
                    }
                }
            });
        if let Some(i) = clicked {
            self.start_scan(i);
        } else if let Some(p) = scan_request {
            self.selected = None;
            self.scan_path(p, 0);
        }

        // ---- central content ----
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(pal.win_bg)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ui, |ui| {
                self.show_central(ui, &pal);
            });
    }
}

impl App {
    fn show_central(&mut self, ui: &mut egui::Ui, pal: &palette::Pal) {
        if self.scanning {
            self.show_scanning(ui, pal);
            return;
        }
        if self.root.is_none() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("Select a drive to scan")
                        .color(pal.subtext)
                        .size(16.0),
                );
            });
            return;
        }

        // Collect navigation/view/search changes while the tree is borrowed,
        // then apply them after the borrow is released.
        let mut nav: Option<NavAction> = None;
        let mut new_view = self.view;
        let mut new_search = self.search.clone();

        {
            let root = self.root.as_ref().unwrap();
            let cur_node = node_at(root, &self.cur);

            // toolbar: nav buttons + breadcrumb
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.back.is_empty(), egui::Button::new("Back"))
                    .clicked()
                {
                    nav = Some(NavAction::Back);
                }
                if ui
                    .add_enabled(!self.fwd.is_empty(), egui::Button::new("Fwd"))
                    .clicked()
                {
                    nav = Some(NavAction::Fwd);
                }
                if ui
                    .add_enabled(!self.cur.is_empty(), egui::Button::new("Up"))
                    .clicked()
                {
                    nav = Some(NavAction::Up);
                }
                ui.separator();
                if ui
                    .link(egui::RichText::new(&root.name).color(pal.blue))
                    .clicked()
                {
                    nav = Some(NavAction::Crumb(0));
                }
                let mut node = root;
                for (depth, &idx) in self.cur.iter().enumerate() {
                    if let Some(child) = node.children.get(idx) {
                        ui.label(egui::RichText::new("/").color(pal.subtext));
                        let is_last = depth + 1 == self.cur.len();
                        if is_last {
                            ui.label(egui::RichText::new(&child.name).color(pal.text).strong());
                        } else if ui
                            .link(egui::RichText::new(&child.name).color(pal.blue))
                            .clicked()
                        {
                            nav = Some(NavAction::Crumb(depth + 1));
                        }
                        node = child;
                    }
                }
            });

            // view tabs + search
            ui.horizontal(|ui| {
                ui.selectable_value(&mut new_view, View::Browse, "Browse");
                ui.selectable_value(&mut new_view, View::Largest, "Largest files");
                ui.selectable_value(&mut new_view, View::Oldest, "Oldest files");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("×").on_hover_text("Clear search").clicked() {
                        new_search.clear();
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut new_search)
                            .hint_text("Search all files…")
                            .desired_width(220.0),
                    );
                });
            });
            ui.add_space(4.0);

            ui.label(
                egui::RichText::new(format!(
                    "{}  ·  {}  ·  {} files",
                    cur_node.full_path,
                    format_bytes(cur_node.size),
                    cur_node.file_count
                ))
                .color(pal.subtext)
                .size(12.0),
            );
            ui.add_space(6.0);

            let terms: Vec<String> = new_search
                .split_whitespace()
                .map(str::to_lowercase)
                .collect();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if !terms.is_empty() {
                    let mut hits: Vec<Hit> = Vec::new();
                    collect_search(root, &terms, &mut hits, 500);
                    hits.sort_by_key(|h| std::cmp::Reverse(h.size));
                    if hits.is_empty() {
                        ui.label(egui::RichText::new("No matches").color(pal.subtext));
                    }
                    for h in &hits {
                        file_row(ui, &h.path, format_bytes(h.size), h.is_dir, pal);
                    }
                } else {
                    match new_view {
                        View::Browse => {
                            let total = cur_node.size.max(1) as f64;
                            let mut kids: Vec<(usize, &FolderNode)> =
                                cur_node.children.iter().enumerate().collect();
                            kids.sort_by_key(|(_, k)| std::cmp::Reverse(k.size));
                            for (idx, k) in kids {
                                let frac = (k.size as f64 / total) as f32;
                                if list_row(ui, &k.name, frac, format_bytes(k.size), true, pal)
                                    .double_clicked()
                                {
                                    nav = Some(NavAction::Drill(idx));
                                }
                            }
                            let mut files: Vec<&FileEntry> = cur_node.files.iter().collect();
                            files.sort_by_key(|f| std::cmp::Reverse(f.size));
                            for f in files {
                                let frac = (f.size as f64 / total) as f32;
                                list_row(ui, &f.name, frac, format_bytes(f.size), false, pal);
                            }
                        }
                        View::Largest => {
                            for h in analysis::top_n_files(root, 300) {
                                file_row(ui, &hit_path(&h), format_bytes(h.file.size), false, pal);
                            }
                        }
                        View::Oldest => {
                            for h in analysis::oldest_n_files(root, 300) {
                                let d = datetime::short_date(h.file.last_modified_ft);
                                let label = format!("{d}   {}", hit_path(&h));
                                file_row(ui, &label, format_bytes(h.file.size), false, pal);
                            }
                        }
                    }
                }
            });
        }

        self.view = new_view;
        self.search = new_search;
        self.apply_nav(nav);
    }

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

// A search hit: a file or folder whose name matched every query term.
struct Hit {
    path: String,
    size: i64,
    is_dir: bool,
}

// Walk the drill path (child indices) to the current node, clamping on any bad
// index (the tree never changes after a scan, so this normally can't miss).
fn node_at<'a>(root: &'a FolderNode, path: &[usize]) -> &'a FolderNode {
    let mut n = root;
    for &i in path {
        match n.children.get(i) {
            Some(c) => n = c,
            None => break,
        }
    }
    n
}

fn name_matches(name: &str, terms: &[String]) -> bool {
    let lower = name.to_lowercase();
    terms.iter().all(|t| lower.contains(t))
}

// Recursively gather files/folders whose name matches every term (space = AND),
// capped at `limit` hits.
fn collect_search(node: &FolderNode, terms: &[String], out: &mut Vec<Hit>, limit: usize) {
    for f in &node.files {
        if out.len() >= limit {
            return;
        }
        if name_matches(&f.name, terms) {
            out.push(Hit {
                path: join_native(&node.full_path, &f.name),
                size: f.size,
                is_dir: false,
            });
        }
    }
    for c in &node.children {
        if out.len() >= limit {
            return;
        }
        if name_matches(&c.name, terms) {
            out.push(Hit {
                path: c.full_path.clone(),
                size: c.size,
                is_dir: true,
            });
        }
        collect_search(c, terms, out, limit);
    }
}

fn join_native(dir: &str, name: &str) -> String {
    std::path::Path::new(dir)
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn hit_path(h: &analysis::FileHit<'_>) -> String {
    join_native(&h.folder.full_path, &h.file.name)
}

// A path row (no %-bar): path text (left, tail-truncated) + size badge (right).
// Used by the Largest/Oldest/Search views.
fn file_row(
    ui: &mut egui::Ui,
    text: &str,
    size: String,
    is_dir: bool,
    pal: &palette::Pal,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 26.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(5), pal.card_bg);
    }
    let pad = 8.0;
    p.text(
        rect.left_center() + egui::vec2(pad, 0.0),
        egui::Align2::LEFT_CENTER,
        shorten(text, 84),
        FontId::proportional(13.0),
        // folders (dirs) get strong text, files muted
        if is_dir { pal.text } else { pal.subtext },
    );
    p.text(
        rect.right_center() + egui::vec2(-pad, 0.0),
        egui::Align2::RIGHT_CENTER,
        size,
        FontId::proportional(13.0),
        pal.blue,
    );
    resp
}

// A browse row: [chevron for folders] name + %-of-parent bar (green) + size
// badge (blue). Folders are drillable via double-click (caller checks the
// returned Response).
fn list_row(
    ui: &mut egui::Ui,
    name: &str,
    frac: f32,
    size: String,
    is_folder: bool,
    pal: &palette::Pal,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 30.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(5), pal.card_bg);
    }
    let pad = 8.0;
    // name (left) — folders in strong text, files muted
    p.text(
        rect.left_center() + egui::vec2(pad + 2.0, 0.0),
        egui::Align2::LEFT_CENTER,
        name,
        FontId::proportional(13.5),
        if is_folder { pal.text } else { pal.subtext },
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
    resp
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
