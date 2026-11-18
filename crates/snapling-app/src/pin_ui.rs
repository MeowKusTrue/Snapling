//! Transparent GPU-backed pin on Unix; RGB-only softbuffer cannot blend with the desktop.
use crate::ui;
use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};
use snapling_overlay::Shot;

pub fn run(shot: Shot) -> Result<(), String> {
    let size = [shot.width as f32, shot.height as f32];
    let opts = eframe::NativeOptions {
        viewport: ui::viewport("Snapling pin", size, [32.0, 32.0])
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "Snapling pin",
        opts,
        Box::new(move |cc| {
            let mut rgba = shot.bgra;
            for px in rgba.as_chunks_mut::<4>().0 {
                if px[3] == 0 {
                    *px = [255; 4];
                } else {
                    px.swap(0, 2);
                }
            }
            let image = ColorImage::from_rgba_unmultiplied(
                [shot.width as usize, shot.height as usize],
                &rgba,
            );
            let tex = cc
                .egui_ctx
                .load_texture("pin", image, TextureOptions::LINEAR);
            Ok(Box::new(Pin {
                tex,
                opacity: 255,
                scale: 1.0,
                size,
            }))
        }),
    )
    .map_err(|e| e.to_string())
}

struct Pin {
    tex: TextureHandle,
    opacity: u8,
    scale: f32,
    size: [f32; 2],
}
impl eframe::App for Pin {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let (scroll, ctrl, close) = ctx.input(|i| {
            (
                i.raw
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                        _ => None,
                    })
                    .sum::<f32>(),
                i.modifiers.ctrl,
                i.key_pressed(egui::Key::Escape) || i.pointer.secondary_clicked(),
            )
        });
        if close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if scroll != 0.0 {
            if ctrl {
                self.opacity = snapling_core::pin::opacity_step(self.opacity, scroll > 0.0);
            } else {
                self.scale =
                    (self.scale * if scroll > 0.0 { 1.1 } else { 1.0 / 1.1 }).clamp(0.2, 4.0);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    self.size[0] * self.scale,
                    self.size[1] * self.scale,
                )));
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let response = ui.add(
                    egui::Image::new(&self.tex)
                        .fit_to_exact_size(ui.available_size())
                        .tint(Color32::from_white_alpha(self.opacity))
                        .sense(egui::Sense::click_and_drag()),
                );
                if response.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if response.double_clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                response.on_hover_text(
                    "Ctrl + колесо — прозрачность · колесо — размер · Esc — закрыть",
                );
            });
    }
}
