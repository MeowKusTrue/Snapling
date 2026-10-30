//! A shot pinned on systems without the Win32 pin: a small always-on-top window. Any press drags
//! it, Esc or a right click closes it. Hyprland decides floating by a window rule on the title.

use crate::Shot;
use std::num::NonZeroU32;
use std::rc::Rc;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

struct App {
    shot: Shot,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
}

impl App {
    fn redraw(&mut self) {
        let (Some(surface), Some(window)) = (&mut self.surface, &self.window) else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        let (sw, bw) = (self.shot.width as usize, size.width as usize);
        let rows = self.shot.height.min(size.height) as usize;
        let cols = sw.min(bw);
        for y in 0..rows {
            let src = &self.shot.bgra[y * sw * 4..(y * sw + cols) * 4];
            let dst = &mut buffer[y * bw..y * bw + cols];
            for (d, s) in dst.iter_mut().zip(src.as_chunks::<4>().0) {
                // Outside a freeform selection the pin shows white, as the clipboard does.
                let px = if s[3] == 0 {
                    [255; 3]
                } else {
                    [s[0], s[1], s[2]]
                };
                *d = u32::from_le_bytes([px[0], px[1], px[2], 0]);
            }
        }
        window.pre_present_notify();
        let _ = buffer.present();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Snapling pin")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(PhysicalSize::new(self.shot.width, self.shot.height));
        let Ok(window) = event_loop.create_window(attrs) else {
            return event_loop.exit();
        };
        let window = Rc::new(window);
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|c| softbuffer::Surface::new(&c, window.clone()));
        let Ok(surface) = surface else {
            return event_loop.exit();
        };
        self.surface = Some(surface);
        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => match button {
                MouseButton::Left => {
                    if let Some(w) = &self.window {
                        let _ = w.drag_window();
                    }
                }
                MouseButton::Right => event_loop.exit(),
                _ => {}
            },
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::Escape) =>
            {
                event_loop.exit();
            }
            _ => {}
        }
    }
}

/// Blocks until the pin is closed.
pub fn run(shot: Shot) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        shot,
        window: None,
        surface: None,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())
}
