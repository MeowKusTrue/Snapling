//! QR results are shown first; neither URLs nor other payloads execute automatically.
use crate::ui;
use eframe::egui;
use snapling_core::i18n::{t, tf};
use std::path::Path;

pub fn spawn(bgra: &[u8], w: u32, h: u32) -> Result<(), String> {
    let texts = snapling_core::qr::decode(bgra, w, h)?;
    let dir = crate::app_dir("CACHE").join("qr");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let path = dir.join(format!(
        "{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    ));
    std::fs::write(
        &path,
        serde_json::to_vec(&texts).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let started = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .arg("--qr-result")
            .arg(&path)
            .spawn()
    });
    match started {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(path);
            Err(e.to_string())
        }
    }
}

pub fn run(file: &Path, copy: fn(&str) -> Result<(), String>, warn: fn(&str)) {
    let read = std::fs::read(file)
        .map_err(|e| e.to_string())
        .and_then(|data| serde_json::from_slice::<Vec<String>>(&data).map_err(|e| e.to_string()));
    if file.parent() == Some(crate::app_dir("CACHE").join("qr").as_path()) {
        let _ = std::fs::remove_file(file);
    }
    let texts = match read {
        Ok(texts) => texts,
        Err(e) => return warn(&e),
    };
    let opts = eframe::NativeOptions {
        viewport: ui::viewport(t("Snapling — QR-код"), [480.0, 400.0], [360.0, 280.0])
            .with_active(true)
            .with_always_on_top(),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "Snapling QR",
        opts,
        Box::new(move |cc| {
            ui::init(cc);
            Ok(Box::new(Results {
                texts,
                copy,
                status: String::new(),
                raised: false,
            }))
        }),
    ) {
        warn(&e.to_string());
    }
}

struct Results {
    texts: Vec<String>,
    copy: fn(&str) -> Result<(), String>,
    status: String,
    raised: bool,
}
impl eframe::App for Results {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.raised {
            ui::focus_result(&ctx, frame);
            self.raised = true;
        }
        ui::preview(&ctx);
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui::frame_edges(&ctx);
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                let r = ui.heading(t("QR-код"));
                ui::drag_area(ui, r.rect);
                if ui.button(t("Закрыть")).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.label(t(
                "Код прочитан локально. Ссылки открываются только по нажатию.",
            ));
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, text) in self.texts.iter().enumerate() {
                    ui.push_id(i, |ui| {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            let mut shown = text.as_str();
                            ui.add(
                                egui::TextEdit::multiline(&mut shown).desired_width(f32::INFINITY),
                            );
                            ui.horizontal(|ui| {
                                if ui.button(t("Копировать текст")).clicked() {
                                    self.status = match (self.copy)(text) {
                                        Ok(()) => t("Скопировано").into(),
                                        Err(e) => tf("Не удалось скопировать: {e}", &[("e", &e)]),
                                    };
                                }
                                if snapling_core::qr::web_link(text) {
                                    ui.hyperlink_to(t("Открыть ссылку"), text);
                                }
                            });
                        });
                    });
                }
            });
            ui.label(&self.status);
        });
    }
}
