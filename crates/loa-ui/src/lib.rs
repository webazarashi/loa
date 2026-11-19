//! loa-ui: the windows a person sees (spec 4-4). Each window runs in its own short-lived
//! process (`loa __ui <kind>`), reads its request as JSON on stdin and answers on stdout.
//!
//! - `approve`: target, before → after, [拒否] [承認]. Closing the window denies. The approve
//!   button only reacts to a pointer click and is disabled for the first moment, so a stray
//!   Enter key or an instant click-through cannot approve.
//! - `paste`: instructions and a masked input. The value goes to stdout (a pipe to the worker).
//! - `notice`: information while the person works in the browser. Closed by the worker.

use eframe::egui;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ARM_DELAY: Duration = Duration::from_millis(1500);

/// Run a window. Returns the process exit code; the answer is written to stdout.
pub fn run(kind: &str, input: &str) -> i32 {
    let req: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(_) => {
            println!("denied");
            return 2;
        }
    };
    let answer = Arc::new(Mutex::new(None::<String>));
    let (title, size) = match kind {
        "approve" => ("loa — 承認が必要です", [560.0, 460.0]),
        "paste" => ("loa — 値の入力", [520.0, 300.0]),
        "notice" => ("loa", [460.0, 200.0]),
        _ => {
            println!("denied");
            return 2;
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(size)
            .with_resizable(true)
            .with_always_on_top(),
        centered: true,
        ..Default::default()
    };
    let a2 = answer.clone();
    let kind_s = kind.to_string();
    let r = eframe::run_native(
        title,
        options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            cc.egui_ctx.set_visuals(egui::Visuals::light());
            Ok(Box::new(Window::new(kind_s, req, a2)))
        }),
    );
    let out = answer.lock().ok().and_then(|g| g.clone());
    match (kind, out, r) {
        ("approve", Some(a), Ok(())) if a == "approved" => {
            println!("approved");
            0
        }
        ("approve", _, _) => {
            println!("denied");
            0
        }
        ("paste", Some(v), Ok(())) => {
            println!("{v}");
            0
        }
        ("paste", _, _) => 1,
        (_, _, Ok(())) => 0,
        _ => 1,
    }
}

struct Window {
    kind: String,
    req: Value,
    answer: Arc<Mutex<Option<String>>>,
    opened: Instant,
    pasted: String,
}

impl Window {
    fn new(kind: String, req: Value, answer: Arc<Mutex<Option<String>>>) -> Self {
        Window {
            kind,
            req,
            answer,
            opened: Instant::now(),
            pasted: String::new(),
        }
    }

    fn finish(&self, ctx: &egui::Context, value: Option<String>) {
        if let Ok(mut g) = self.answer.lock() {
            *g = value;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn s<'a>(&'a self, key: &str) -> &'a str {
        self.req.get(key).and_then(Value::as_str).unwrap_or("")
    }

    fn approve_ui(&mut self, ui: &mut egui::Ui) {
        let kind = self.s("kind").to_string();
        let badge = match kind.as_str() {
            "terms" => "利用規約への同意",
            "bulk" => "まとめて承認",
            _ => "変更の承認",
        };
        ui.label(
            egui::RichText::new(badge)
                .small()
                .color(egui::Color32::from_rgb(160, 60, 20)),
        );
        ui.heading(self.s("title"));
        ui.add_space(6.0);
        if let Some(t) = self.req.get("target").and_then(Value::as_array) {
            egui::Grid::new("target")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for row in t {
                        let a = row.get(0).and_then(Value::as_str).unwrap_or("");
                        let b = row.get(1).and_then(Value::as_str).unwrap_or("");
                        ui.label(egui::RichText::new(a).strong());
                        ui.label(b);
                        ui.end_row();
                    }
                });
        }
        if let Some(ch) = self.req.get("changes").and_then(Value::as_array)
            && !ch.is_empty()
        {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("変更内容").strong());
            egui::Grid::new("changes")
                .num_columns(3)
                .striped(true)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    ui.label("項目");
                    ui.label("変更前");
                    ui.label("変更後");
                    ui.end_row();
                    for c in ch {
                        let f = |k: &str| {
                            c.get(k)
                                .and_then(Value::as_str)
                                .unwrap_or("（なし）")
                                .to_string()
                        };
                        ui.label(f("field"));
                        ui.label(f("before"));
                        ui.label(egui::RichText::new(f("after")).strong());
                        ui.end_row();
                    }
                });
        }
        if let Some(link) = self.req.get("link").and_then(Value::as_str) {
            ui.add_space(6.0);
            ui.hyperlink_to("規約・詳細を開く", link);
        }
        if let Some(n) = self.req.get("note").and_then(Value::as_str) {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(n).italics());
        }
        ui.add_space(12.0);
        let armed = self.opened.elapsed() >= ARM_DELAY;
        ui.horizontal(|ui| {
            if ui
                .add_sized([120.0, 32.0], egui::Button::new("拒否"))
                .clicked()
            {
                self.finish(ui.ctx(), Some("denied".into()));
            }
            ui.add_space(16.0);
            let approve = egui::Button::new(egui::RichText::new("承認").strong())
                .fill(egui::Color32::from_rgb(200, 230, 200));
            let resp = ui
                .add_enabled_ui(armed, |ui| ui.add_sized([120.0, 32.0], approve))
                .inner;
            // pointer clicks only: keyboard activation does not approve
            if resp.clicked_by(egui::PointerButton::Primary) {
                self.finish(ui.ctx(), Some("approved".into()));
            }
        });
        if !armed {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    fn paste_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.s("title"));
        ui.add_space(6.0);
        ui.label(self.s("instructions"));
        ui.add_space(8.0);
        ui.label(egui::RichText::new(self.s("field_label").to_string()).strong());
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.pasted)
                .password(true)
                .desired_width(f32::INFINITY),
        );
        resp.request_focus();
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("キャンセル").clicked() {
                self.finish(ui.ctx(), None);
            }
            let ok = ui.add_enabled(!self.pasted.trim().is_empty(), egui::Button::new("保存"));
            if ok.clicked() {
                let v = std::mem::take(&mut self.pasted);
                self.finish(ui.ctx(), Some(v.trim().to_string()));
            }
        });
    }

    fn notice_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading(self.s("title"));
        ui.add_space(6.0);
        ui.label(self.s("message"));
        ui.add_space(6.0);
        ui.label(egui::RichText::new("完了するとこのウィンドウは自動で閉じます。").small());
        if ui.button("閉じる").clicked() {
            self.finish(ui.ctx(), None);
        }
    }
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.kind.as_str() {
                "approve" => self.approve_ui(ui),
                "paste" => self.paste_ui(ui),
                _ => self.notice_ui(ui),
            });
        });
    }
}

/// Japanese glyphs come from the OS (Hiragino on macOS, Yu Gothic / Meiryo on Windows).
fn install_fonts(ctx: &egui::Context) {
    let candidates: &[(&str, u32)] = if cfg!(target_os = "macos") {
        &[
            ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
            ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
            ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
            ("/Library/Fonts/Arial Unicode.ttf", 0),
        ]
    } else {
        &[
            ("C:\\Windows\\Fonts\\YuGothM.ttc", 0),
            ("C:\\Windows\\Fonts\\meiryo.ttc", 0),
            ("C:\\Windows\\Fonts\\msgothic.ttc", 0),
        ]
    };
    let mut fonts = egui::FontDefinitions::default();
    for (path, index) in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut data = egui::FontData::from_owned(bytes);
            data.index = *index;
            fonts.font_data.insert("jp".into(), Arc::new(data));
            for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push("jp".into());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}
