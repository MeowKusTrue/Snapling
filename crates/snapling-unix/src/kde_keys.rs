//! KDE's native accelerator service. Config is authoritative; never steal another app's key.
use futures_lite::StreamExt;
use snapling_core::config::{Action, Config, Hotkeys};
use snapling_core::hotkey::{self, Hotkey};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use zbus::{Connection, Proxy};

const COMPONENT: &str = "app.snapling.Snapling";
const SERVICE: &str = "org.kde.kglobalaccel";
const INTERFACE: &str = "org.kde.KGlobalAccel";
const TICK: Duration = Duration::from_millis(500);

pub fn action_flag(a: Action) -> Option<&'static str> {
    Some(match a {
        Action::CaptureRegion => "--region",
        Action::CaptureScreen => "--screen",
        Action::CaptureWindow => "--window",
        Action::DelayedRegion => "--delay",
        Action::RecognizeText => "--ocr",
        Action::TranslateText => "--ocr-translate",
        _ => return None,
    })
}

/// Qt combined key codes, the wire format of KGlobalAccel's QKeySequence.
pub fn qt_key(h: &Hotkey) -> Result<i32, String> {
    let k = h.key.as_str();
    let key = if k.len() == 1 {
        i32::from(k.as_bytes()[0])
    } else if let Some(n) = k.strip_prefix('F').and_then(|s| s.parse::<i32>().ok()) {
        0x01000030 + n - 1
    } else {
        match k {
            "Escape" => 0x01000000,
            "Tab" => 0x01000001,
            "Backspace" => 0x01000003,
            "Enter" => 0x01000004,
            "Insert" => 0x01000006,
            "Delete" => 0x01000007,
            "Pause" => 0x01000008,
            "PrintScreen" => 0x01000009,
            "Home" => 0x01000010,
            "End" => 0x01000011,
            "Left" => 0x01000012,
            "Up" => 0x01000013,
            "Right" => 0x01000014,
            "Down" => 0x01000015,
            "PageUp" => 0x01000016,
            "PageDown" => 0x01000017,
            "CapsLock" => 0x01000024,
            "NumLock" => 0x01000025,
            "ScrollLock" => 0x01000026,
            "Space" => 0x20,
            "Semicolon" => 0x3b,
            "Equal" => 0x3d,
            "Comma" => 0x2c,
            "Minus" => 0x2d,
            "Period" => 0x2e,
            "Slash" => 0x2f,
            "Backquote" => 0x60,
            "BracketLeft" => 0x5b,
            "Backslash" => 0x5c,
            "BracketRight" => 0x5d,
            "Quote" => 0x27,
            _ => return Err(format!("KDE: клавиша {k} не поддерживается")),
        }
    };
    Ok(key
        | if h.shift { 0x02000000 } else { 0 }
        | if h.ctrl { 0x04000000 } else { 0 }
        | if h.alt { 0x08000000 } else { 0 }
        | if h.win { 0x10000000 } else { 0 })
}

fn id(a: Action) -> [&'static str; 4] {
    [COMPONENT, a.key(), "Snapling", a.title()]
}

async fn proxy(c: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(c, SERVICE, "/kglobalaccel", INTERFACE).await
}

async fn clear(p: &Proxy<'_>) -> zbus::Result<()> {
    for a in Action::ALL {
        let _: bool = p.call("unregister", &(COMPONENT, a.key())).await?;
    }
    Ok(())
}

async fn bind(p: &Proxy<'_>, h: &Hotkeys) -> zbus::Result<Vec<(Action, i32)>> {
    // Remove the complete old set first, including disabled and unsupported actions.
    clear(p).await?;
    let (keys, mut errors) = hotkey::check(h, |_| None);
    let mut active = Vec::new();
    for (a, key) in keys {
        if action_flag(a).is_none() {
            errors.push(format!(
                "{}: действие пока не поддерживается в KDE/Wayland",
                a.title()
            ));
            continue;
        }
        let qt = match qt_key(&key) {
            Ok(qt) => qt,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let available: bool = p
            .call("globalShortcutAvailable", &((vec![qt],), COMPONENT))
            .await?;
        if !available {
            errors.push(format!("{key}: занято в KDE. Измени сочетание в Snapling или освободи его в настройках KDE."));
            continue;
        }
        p.call::<_, _, ()>("doRegister", &(id(a),)).await?;
        // SetPresent | NoAutoloading: apply the new config rather than daemon defaults.
        let actual: Vec<(Vec<i32>,)> = p
            .call("setShortcutKeys", &(id(a), vec![(vec![qt],)], 6u32))
            .await?;
        if actual == vec![(vec![qt],)] {
            active.push((a, qt));
        } else {
            let _: bool = p.call("unregister", &(COMPONENT, a.key())).await?;
            errors.push(format!("{key}: KDE отказал в регистрации ({actual:?})"));
        }
    }
    if !errors.is_empty() {
        report(&errors.join("\n"));
    }
    Ok(active)
}

fn report(e: &str) {
    eprintln!("Snapling KDE: {e}");
    crate::notify("Snapling — горячие клавиши KDE", e);
}

pub struct Keys {
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    // Dedicated name owner prevents a second tray from unregistering the first one's keys.
    _instance: zbus::blocking::Connection,
    signals: Vec<signal_hook_registry::SigId>,
}

impl Keys {
    pub fn start(path: PathBuf) -> Result<Self, String> {
        let instance = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.method_timeout(Duration::from_secs(3)).name(COMPONENT))
            .and_then(|b| b.build())
            .map_err(|e| format!("KDE: Snapling уже запущен либо D-Bus недоступен: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let mut signals = Vec::new();
        for signal in [2, 15] {
            // Linux SIGINT and SIGTERM.
            let flag = stop.clone();
            // Only a lock-free atomic store runs in the signal handler.
            let id = unsafe {
                signal_hook_registry::register(signal, move || flag.store(true, Ordering::Relaxed))
            };
            match id {
                Ok(id) => signals.push(id),
                Err(e) => {
                    for id in signals {
                        signal_hook_registry::unregister(id);
                    }
                    return Err(format!("KDE: cannot install shutdown handler: {e}"));
                }
            }
        }
        let flag = stop.clone();
        let worker = std::thread::spawn(move || futures_lite::future::block_on(worker(path, flag)));
        Ok(Self {
            stop,
            worker: Some(worker),
            _instance: instance,
            signals,
        })
    }

    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        for id in self.signals.drain(..) {
            signal_hook_registry::unregister(id);
        }
    }
}

async fn worker(path: PathBuf, stop: Arc<AtomicBool>) {
    let mut last_error = String::new();
    while !stop.load(Ordering::Relaxed) {
        if let Err(e) = session(&path, &stop).await {
            let e = format!(
                "KGlobalAccel: {e}. Проверь D-Bus и службу KDE; регистрация не подтверждена."
            );
            if e != last_error {
                report(&e);
                last_error = e;
            }
        }
        async_io::Timer::after(Duration::from_secs(2)).await;
    }
}

async fn session(path: &std::path::Path, stop: &AtomicBool) -> zbus::Result<()> {
    let c = zbus::connection::Builder::session()?
        .method_timeout(Duration::from_secs(5))
        .build()
        .await?;
    let p = proxy(&c).await?;
    clear(&p).await?;
    // Always create the component before subscribing, even when every binding is disabled.
    p.call::<_, _, ()>("doRegister", &(id(Action::CaptureRegion),))
        .await?;
    let component: zbus::zvariant::OwnedObjectPath = p.call("getComponent", &(COMPONENT,)).await?;
    let cp = Proxy::new(&c, SERVICE, component, "org.kde.kglobalaccel.Component").await?;
    let mut pressed = cp.receive_signal("globalShortcutPressed").await?;
    let dbus = zbus::fdo::DBusProxy::new(&c).await?;
    let service = zbus::names::BusName::try_from(SERVICE)?;
    let owner = dbus.get_name_owner(service.clone()).await?;
    let mut loaded = None;
    let mut active = Vec::new();
    let mut problem = String::new();
    let mut revoked = Vec::new();
    let result = async {
        while !stop.load(Ordering::Relaxed) {
            if dbus.get_name_owner(service.clone()).await? != owner {
                return Err(zbus::Error::Failure("служба KDE перезапущена".into()));
            }
            match Config::load_or_create(path) {
                Ok(cfg) if loaded.as_ref() != Some(&cfg.hotkeys) => {
                    active = bind(&p, &cfg.hotkeys).await?;
                    loaded = Some(cfg.hotkeys);
                    problem.clear();
                    revoked.clear();
                }
                Err(e) if e != problem => { report(&e); problem = e; }
                _ => {}
            }
            // Detect revocation or external reassignment; never claim the old combo still works.
            for (a, qt) in &active {
                let actual: Vec<(Vec<i32>,)> = p.call("shortcutKeys", &(id(*a),)).await?;
                if actual != vec![(vec![*qt],)] {
                    let e = format!("{}: сочетание изменено или отключено в KDE. Измени его в настройках Snapling для повторной регистрации.", a.title());
                    if !revoked.contains(a) { report(&e); revoked.push(*a); }
                }
            }
            let event = futures_lite::future::race(
                async { pressed.next().await },
                async { async_io::Timer::after(TICK).await; None },
            ).await;
            if let Some(msg) = event {
                let (component, key, _): (String, String, i64) = msg.body().deserialize()?;
                if component == COMPONENT
                    && let Some((a, _)) = active.iter().find(|(a, _)| a.key() == key)
                    && !revoked.contains(a)
                    && let Some(flag) = action_flag(*a)
                    && let Ok(exe) = std::env::current_exe()
                {
                    match std::process::Command::new(exe).arg(flag).spawn() {
                        Ok(mut child) => { std::thread::spawn(move || { let _ = child.wait(); }); }
                        Err(e) => report(&e.to_string()),
                    }
                }
            }
        }
        Ok(())
    }.await;
    // Also clean up after a partial registration or a denied D-Bus request.
    if let Err(e) = clear(&p).await {
        report(&format!("Не удалось снять регистрации: {e}"));
    }
    result
}

/// Read-only capability probe; does not register or request permission.
pub fn probe() -> Result<(bool, String), String> {
    let c = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.method_timeout(Duration::from_secs(3)).build())
        .map_err(|e| e.to_string())?;
    let p = zbus::blocking::Proxy::new(&c, SERVICE, "/kglobalaccel", INTERFACE)
        .map_err(|e| e.to_string())?;
    let _: bool = p
        .call(
            "globalShortcutAvailable",
            &((vec![0x01000048i32],), COMPONENT),
        )
        .map_err(|e| e.to_string())?;
    let bus = zbus::blocking::fdo::DBusProxy::new(&c).map_err(|e| e.to_string())?;
    let live = bus
        .name_has_owner(zbus::names::BusName::try_from(COMPONENT).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if !live {
        return Ok((
            false,
            "KGlobalAccel available; Snapling tray is not running, no live bindings confirmed"
                .into(),
        ));
    }
    let mut bindings = Vec::new();
    for a in Action::ALL {
        let keys: Vec<(Vec<i32>,)> = p
            .call("shortcutKeys", &(id(a),))
            .map_err(|e| e.to_string())?;
        if !keys.is_empty() {
            bindings.push(format!("{}={keys:x?}", a.key()));
        }
    }
    Ok((
        !bindings.is_empty(),
        format!(
            "KGlobalAccel; daemon bindings (Qt key codes): {}; test actual key activation separately",
            bindings.join(", ")
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn qt_codes_preserve_modifiers_and_reject_windows_and_mouse_keys() {
        assert_eq!(
            qt_key(&Hotkey::parse("Ctrl+Shift+PrintScreen").unwrap()).unwrap(),
            0x07000009
        );
        assert_eq!(
            qt_key(&Hotkey::parse("Win+Alt+F24").unwrap()).unwrap(),
            0x19000047
        );
        assert_eq!(
            qt_key(&Hotkey::parse("Ctrl+A").unwrap()).unwrap(),
            0x04000041
        );
        for key in ["Mouse4", "VkFF"] {
            assert!(qt_key(&Hotkey::parse(key).unwrap()).is_err());
        }
        assert_eq!(action_flag(Action::CaptureWindow), Some("--window"));
        assert_eq!(action_flag(Action::RepeatRegion), None);
    }

    #[derive(Clone, Default)]
    struct FakeAccel(Arc<std::sync::Mutex<Registered>>);

    type Registered = std::collections::HashMap<String, Vec<(Vec<i32>,)>>;

    #[zbus::interface(name = "org.kde.KGlobalAccel")]
    impl FakeAccel {
        #[zbus(name = "unregister")]
        fn unregister(&self, _component: &str, action: &str) -> bool {
            self.0.lock().unwrap().remove(action).is_some()
        }
        #[zbus(name = "doRegister")]
        fn register(&self, action: Vec<String>) {
            self.0.lock().unwrap().entry(action[1].clone()).or_default();
        }
        #[zbus(name = "globalShortcutAvailable")]
        fn available(&self, key: (Vec<i32>,), _component: &str) -> bool {
            key.0 != vec![0x04000058] // Ctrl+X belongs to another app.
        }
        #[zbus(name = "setShortcutKeys")]
        fn set(
            &self,
            action: Vec<String>,
            keys: Vec<(Vec<i32>,)>,
            flags: u32,
        ) -> zbus::fdo::Result<Vec<(Vec<i32>,)>> {
            assert_eq!(flags, 6);
            if keys == vec![(vec![0x04000059],)] {
                // Ctrl+Y is denied by policy.
                return Err(zbus::fdo::Error::AccessDenied("denied by test".into()));
            }
            self.0
                .lock()
                .unwrap()
                .insert(action[1].clone(), keys.clone());
            Ok(keys)
        }
    }

    #[test]
    #[ignore = "run under a private bus: dbus-run-session -- cargo test -p snapling-unix private_bus -- --ignored"]
    fn private_bus_registration_rebind_disable_conflict_and_denial() {
        futures_lite::future::block_on(async {
            let fake = FakeAccel::default();
            let state = fake.0.clone();
            // Refuses to take an already owned service name, protecting a real KDE session.
            let _server = zbus::connection::Builder::session()
                .unwrap()
                .name(SERVICE)
                .unwrap()
                .serve_at("/kglobalaccel", fake)
                .unwrap()
                .build()
                .await
                .unwrap();
            let client = Connection::session().await.unwrap();
            let p = proxy(&client).await.unwrap();
            let mut cfg = Hotkeys::default();
            for a in Action::ALL {
                cfg.set_combo(a, String::new());
            }
            cfg.capture_region = "Ctrl+A".into();
            assert_eq!(
                bind(&p, &cfg).await.unwrap(),
                vec![(Action::CaptureRegion, 0x04000041)]
            );
            cfg.capture_region.clear();
            cfg.capture_screen = "Ctrl+B".into();
            assert_eq!(
                bind(&p, &cfg).await.unwrap(),
                vec![(Action::CaptureScreen, 0x04000042)]
            );
            assert!(!state.lock().unwrap().contains_key("capture_region"));
            cfg.capture_region = "Ctrl+X".into();
            assert_eq!(bind(&p, &cfg).await.unwrap().len(), 1);
            assert!(!state.lock().unwrap().contains_key("capture_region"));
            cfg.capture_region = "Ctrl+Y".into();
            assert!(bind(&p, &cfg).await.is_err());
            clear(&p).await.unwrap();
            assert!(state.lock().unwrap().is_empty());
            cfg.capture_region.clear();
            cfg.capture_screen.clear();
            assert!(bind(&p, &cfg).await.unwrap().is_empty());
            assert!(state.lock().unwrap().is_empty());
        });
    }
}
