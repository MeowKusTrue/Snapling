//! Selection overlay on winit and softbuffer, for systems without the Win32 one: a fullscreen
//! window showing the shot. All behavior is in [`Session`]; this file only feeds it events.

mod session;

mod pin;

pub use pin::run as pin;
pub use session::{Key, Mods, Options, Outcome, Pointer, Session, Shot, Step};

use snapling_core::selection::Point;
use std::num::NonZeroU32;
use std::rc::Rc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Fullscreen, Window, WindowId};

const DOUBLE_CLICK_MS: u128 = 400;

struct App {
    session: Session,
    width: u32,
    height: u32,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    back: Vec<u8>,
    mods: Mods,
    last_click: Option<std::time::Instant>,
    done: Option<Step>,
}

fn pointer(p: Pointer) -> CursorIcon {
    match p {
        Pointer::Cross => CursorIcon::Crosshair,
        Pointer::Hand => CursorIcon::Pointer,
        Pointer::Move => CursorIcon::Move,
        Pointer::Text => CursorIcon::Text,
        Pointer::ResizeNwSe => CursorIcon::NwseResize,
        Pointer::ResizeNeSw => CursorIcon::NeswResize,
    }
}

fn map_key(key: &WinitKey) -> Option<Key> {
    match key {
        WinitKey::Named(NamedKey::Escape) => Some(Key::Escape),
        WinitKey::Named(NamedKey::Enter) => Some(Key::Enter),
        WinitKey::Named(NamedKey::Backspace) => Some(Key::Backspace),
        WinitKey::Named(NamedKey::ArrowLeft) => Some(Key::Left),
        WinitKey::Named(NamedKey::ArrowRight) => Some(Key::Right),
        WinitKey::Named(NamedKey::ArrowUp) => Some(Key::Up),
        WinitKey::Named(NamedKey::ArrowDown) => Some(Key::Down),
        WinitKey::Named(NamedKey::Space) => Some(Key::Char(' ')),
        WinitKey::Character(s) => s.chars().next().map(Key::Char),
        _ => None,
    }
}

impl App {
    fn finish(&mut self, step: Step, event_loop: &ActiveEventLoop) {
        if step != Step::Stay {
            self.done = Some(step);
            event_loop.exit();
        } else if let Some(w) = &self.window {
            w.set_cursor(pointer(self.session.cursor()));
            w.request_redraw();
        }
    }

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
        self.session.render(&mut self.back);
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        // The shot and the window can differ by a few pixels: a bar on the screen, a rounding.
        let (cw, ch) = (
            size.width.min(self.width) as usize,
            size.height.min(self.height) as usize,
        );
        let (bw, sw) = (size.width as usize, self.width as usize);
        for y in 0..ch {
            let src = &self.back[y * sw * 4..(y * sw + cw) * 4];
            let dst = &mut buffer[y * bw..y * bw + cw];
            for (d, s) in dst.iter_mut().zip(src.as_chunks::<4>().0) {
                *d = u32::from_le_bytes([s[0], s[1], s[2], 0]);
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
            .with_title("Snapling")
            .with_fullscreen(Some(Fullscreen::Borderless(None)))
            .with_decorations(false);
        let Ok(window) = event_loop.create_window(attrs) else {
            self.done = Some(Step::Cancel);
            return event_loop.exit();
        };
        let window = Rc::new(window);
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|c| softbuffer::Surface::new(&c, window.clone()));
        match surface {
            Ok(s) => self.surface = Some(s),
            Err(_) => {
                self.done = Some(Step::Cancel);
                return event_loop.exit();
            }
        }
        self.session.set_scale(window.scale_factor() as f32);
        window.set_cursor(CursorIcon::Crosshair);
        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.finish(Step::Cancel, event_loop),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ModifiersChanged(m) => {
                let state: ModifiersState = m.state();
                let shift = state.shift_key();
                if shift != self.mods.shift {
                    self.session.shift(shift);
                }
                self.mods = Mods {
                    shift,
                    ctrl: state.control_key(),
                };
                self.finish(Step::Stay, event_loop);
            }
            WindowEvent::CursorMoved { position, .. } => {
                let p = Point {
                    x: position.x as i32,
                    y: position.y as i32,
                };
                self.session.moved(p, self.mods);
                self.finish(Step::Stay, event_loop);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let step = match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        let now = std::time::Instant::now();
                        let double = self
                            .last_click
                            .is_some_and(|t| t.elapsed().as_millis() < DOUBLE_CLICK_MS);
                        self.last_click = Some(now);
                        let p = self.session.mouse();
                        if double {
                            self.session.double_click(p)
                        } else {
                            self.session.press(p, self.mods)
                        }
                    }
                    (MouseButton::Left, ElementState::Released) => self.session.release(),
                    (MouseButton::Right | MouseButton::Middle, ElementState::Pressed) => {
                        self.session.cancel_click()
                    }
                    _ => Step::Stay,
                };
                self.finish(step, event_loop);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.logical_key == WinitKey::Named(NamedKey::Space) {
                    self.session.set_pan(event.state == ElementState::Pressed);
                }
                if event.state == ElementState::Pressed
                    && let Some(key) = map_key(&event.logical_key)
                {
                    let step = self.session.key(key, self.mods);
                    self.finish(step, event_loop);
                }
            }
            _ => {}
        }
    }
}

/// What an overlay ended with, and what its panels should keep for the next one.
pub struct Ended {
    pub result: Option<(Shot, Outcome)>,
    pub blur: snapling_core::annotate::BlurStyle,
    pub picker: bool,
    pub color_format: snapling_core::color::ColorFormat,
}

/// Shows the shot and blocks until the user is done.
pub fn run(shot: Shot, opts: Options) -> Result<Ended, String> {
    let (width, height) = (shot.width, shot.height);
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        session: Session::new(shot, opts),
        width,
        height,
        window: None,
        surface: None,
        back: vec![0; width as usize * height as usize * 4],
        mods: Mods::default(),
        last_click: None,
        done: None,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    let (blur, picker, color_format) = app.session.prefs();
    let result = match app.done {
        Some(Step::Done(out)) => app.session.result(out).map(|img| (img, out)),
        _ => None,
    };
    Ok(Ended {
        result,
        blur,
        picker,
        color_format,
    })
}
