//! User-owned Linux installs: atomic replacement and a private, graceful tray restart.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn socket_dir() -> Result<PathBuf, String> {
    let uid = unsafe { libc::geteuid() };
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = runtime.join(format!("snapling-{uid}"));
    match fs::create_dir(&dir) {
        Ok(()) => fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(error)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(error(e)),
    }
    let m = fs::symlink_metadata(&dir).map_err(error)?;
    if !m.is_dir() || m.uid() != uid || m.mode() & 0o077 != 0 {
        return Err("Небезопасный каталог управляющего сокета Snapling".into());
    }
    Ok(dir)
}
fn socket() -> Result<PathBuf, String> {
    Ok(socket_dir()?.join("control.sock"))
}
fn request(path: &Path, message: &str) -> Result<String, String> {
    let mut stream = UnixStream::connect(path).map_err(error)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(error)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(error)?;
    stream.write_all(message.as_bytes()).map_err(error)?;
    let mut response = String::new();
    stream
        .take(80)
        .read_to_string(&mut response)
        .map_err(error)?;
    Ok(response.trim().into())
}

pub struct Instance {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct UpdateLock(PathBuf);
impl UpdateLock {
    fn acquire() -> Result<Self, String> {
        let path = socket_dir()?.join("update.lock");
        if path.exists() {
            let owner = fs::read_to_string(&path).map_err(error)?;
            let pid = owner.trim().parse::<u32>().map_err(error)?;
            if Path::new("/proc").join(pid.to_string()).exists() {
                return Err("Обновление Snapling уже выполняется".into());
            }
            fs::remove_file(&path).map_err(error)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(error)?;
        writeln!(file, "{}", std::process::id()).map_err(error)?;
        Ok(Self(path))
    }
}
impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
impl Instance {
    /// `None` means a healthy tray already owns this user's private socket.
    pub fn acquire() -> Result<Option<Self>, String> {
        let path = socket()?;
        if request(&path, "ping\n").is_ok() {
            return Ok(None);
        }
        if path.exists() {
            fs::remove_file(&path).map_err(error)?;
        }
        let listener = UnixListener::bind(&path).map_err(error)?;
        listener.set_nonblocking(true).map_err(error)?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let mut buf = [0u8; 32];
                    if let Ok(n) = stream.read(&mut buf) {
                        match &buf[..n] {
                            b"ping\n" => {
                                let _ = writeln!(stream, "{}", std::process::id());
                            }
                            b"quit-for-update\n" => {
                                let _ = stream.write_all(b"OK\n");
                                flag.store(true, Ordering::Relaxed);
                            }
                            _ => {}
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        Ok(Some(Self {
            path,
            stop,
            thread: Some(thread),
        }))
    }
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}

fn eligible_path(exe: &Path, home: &Path) -> bool {
    exe.starts_with(home.join(".local/bin")) || exe.starts_with(home.join(".local/lib/snapling"))
}
pub fn can_update() -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Ok(m) = fs::metadata(&exe) else {
        return false;
    };
    eligible_path(&exe, &home)
        && m.uid() == unsafe { libc::geteuid() }
        && std::env::var_os("FLATPAK_ID").is_none()
        && std::env::var_os("APPIMAGE").is_none()
}
fn service_group(pid: u32) -> Option<String> {
    let group = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    group
        .lines()
        .any(|line| {
            line.rsplit('/')
                .next()
                .is_some_and(|p| p.ends_with(".service"))
        })
        .then_some(group)
}
fn busy(exe: &Path, daemon: Option<u32>, caller: Option<u32>) -> bool {
    let group = daemon.and_then(service_group);
    let Ok(entries) = fs::read_dir("/proc") else {
        return true;
    };
    entries.flatten().any(|entry| {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            return false;
        };
        let active = fs::read_to_string(entry.path().join("status"))
            .ok()
            .and_then(|status| {
                status.lines().find_map(|line| {
                    line.strip_prefix("State:")
                        .and_then(|value| value.split_whitespace().next())
                        .map(|state| state != "Z" && state != "X")
                })
            })
            .unwrap_or(false);
        active
            && pid != std::process::id()
            && Some(pid) != daemon
            && Some(pid) != caller
            && (fs::read_link(entry.path().join("exe")).is_ok_and(|p| p == exe)
                || group.as_ref().is_some_and(|g| {
                    fs::read_to_string(entry.path().join("cgroup")).is_ok_and(|p| &p == g)
                }))
    })
}
fn verify_executable(path: &Path) -> Result<(), String> {
    let mut file = File::open(path).map_err(error)?;
    if file.metadata().map_err(error)?.len() > 512 * 1024 * 1024 {
        return Err("Слишком большой файл обновления".into());
    }
    let mut header = [0u8; 20];
    file.read_exact(&mut header).map_err(error)?;
    let machine = if cfg!(target_arch = "x86_64") {
        62
    } else if cfg!(target_arch = "aarch64") {
        183
    } else {
        0
    };
    if &header[..4] != b"\x7fELF"
        || header[4] != 2
        || header[5] != 1
        || machine == 0
        || u16::from_le_bytes([header[18], header[19]]) != machine
    {
        return Err("Обновление не соответствует Linux/архитектуре этого компьютера".into());
    }
    Ok(())
}
fn copy_new(source: &Path, dest: &Path) -> Result<(), String> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(dest)
        .map_err(error)?;
    let result = (|| {
        std::io::copy(&mut File::open(source).map_err(error)?, &mut output).map_err(error)?;
        output
            .set_permissions(fs::Permissions::from_mode(0o755))
            .map_err(error)?;
        output.sync_all().map_err(error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}
fn replace(source: &Path, target: &Path) -> Result<PathBuf, String> {
    let parent = target.parent().ok_or("Нет каталога установки")?;
    let tmp = parent.join(format!(".snapling-update-{}.new", std::process::id()));
    let backup_tmp = parent.join(format!(".snapling-update-{}.backup", std::process::id()));
    let backup = parent.join(".snapling-update-previous");
    let result = (|| {
        copy_new(source, &tmp)?;
        copy_new(target, &backup_tmp)?;
        fs::rename(&backup_tmp, &backup).map_err(error)?;
        fs::rename(&tmp, target).map_err(error)?;
        File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(error)?;
        Ok(backup)
    })();
    let _ = fs::remove_file(tmp);
    let _ = fs::remove_file(backup_tmp);
    result
}

fn unit(exe: &Path, args: &[&std::ffi::OsStr], worker: bool) -> Result<String, String> {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(error)?
        .as_nanos();
    let name = format!(
        "snapling-{}-{suffix}",
        if worker { "updater" } else { "tray" }
    );
    let mut cmd = Command::new("systemd-run");
    cmd.args(["--user", "--collect", "--unit", &name]);
    // systemd-run locates the user manager through the real runtime directory.
    // Preserve a custom application's runtime directory only for its child.
    cmd.env(
        "XDG_RUNTIME_DIR",
        format!("/run/user/{}", unsafe { libc::geteuid() }),
    );
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let mut option = std::ffi::OsString::from("--setenv=XDG_RUNTIME_DIR=");
        option.push(runtime);
        cmd.arg(option);
    }
    // Import values without placing their contents in process arguments.
    for key in [
        "HOME",
        "PATH",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_CURRENT_DESKTOP",
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        if std::env::var_os(key).is_some() {
            cmd.arg(format!("--setenv={key}"));
        }
    }
    if worker {
        cmd.arg("--setenv=SNAPLING_UPDATE_WORKER=1");
        cmd.arg(format!(
            "--setenv=SNAPLING_UPDATE_CALLER_PID={}",
            std::process::id()
        ));
    }
    let result = cmd.arg("--").arg(exe).args(args).output().map_err(error)?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).into_owned());
    }
    Ok(name)
}
struct Started {
    unit: Option<String>,
    child: Option<std::process::Child>,
}
impl Started {
    fn exited(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|c| c.try_wait().ok().flatten().is_some())
    }
    fn stop(&mut self) {
        if let Some(name) = &self.unit {
            let _ = Command::new("systemctl")
                .args(["--user", "stop", name])
                .output();
        }
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn start_tray(target: &Path) -> Result<Started, String> {
    if Path::new("/run/systemd/system").is_dir() {
        Ok(Started {
            unit: Some(unit(target, &[], false)?),
            child: None,
        })
    } else {
        use std::os::unix::process::CommandExt;
        let child = Command::new(target)
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(error)?;
        Ok(Started {
            unit: None,
            child: Some(child),
        })
    }
}

/// The settings/check process owns this operation; other editors/captures block it.
pub fn install(staged: &Path) -> Result<(), String> {
    if !can_update() {
        return Err(
            "Эта установка обновляется через пакетный менеджер или исходный способ установки"
                .into(),
        );
    }
    // Do not let the old tray service kill the updater when its main PID exits.
    if service_group(std::process::id()).is_some()
        && std::env::var_os("SNAPLING_UPDATE_WORKER").is_none()
    {
        unit(
            &std::env::current_exe().map_err(error)?,
            &[std::ffi::OsStr::new("--apply-update"), staged.as_os_str()],
            true,
        )?;
        return Ok(());
    }
    let lock = UpdateLock::acquire()?;
    if super::recording().is_some() {
        return Err("Сначала заверши запись экрана".into());
    }
    let target = std::env::current_exe().map_err(error)?;
    let control = socket()?;
    let daemon = request(&control, "ping\n")
        .ok()
        .and_then(|s| s.parse::<u32>().ok());
    let caller = std::env::var("SNAPLING_UPDATE_CALLER_PID")
        .ok()
        .and_then(|s| s.parse().ok());
    if busy(&target, daemon, caller) {
        return Err("Закрой другие окна Snapling и заверши захват перед обновлением".into());
    }
    verify_executable(staged)?;
    fs::set_permissions(staged, fs::Permissions::from_mode(0o700)).map_err(error)?;
    let version = Command::new(staged)
        .arg("--version")
        .output()
        .map_err(error)?;
    if !version.status.success() || !version.stdout.starts_with(b"snapling ") {
        return Err("Проверочный запуск обновления не удался".into());
    }
    // No executable is replaced before all captures/editors have been checked.
    if daemon.is_some() {
        request(&control, "quit-for-update\n")?;
        for _ in 0..100 {
            if !control.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if control.exists() {
            return Err("Не удалось безопасно остановить Snapling".into());
        }
    }
    let backup = match replace(staged, &target) {
        Ok(p) => p,
        Err(e) => {
            if daemon.is_some() {
                let _ = start_tray(&target);
            }
            return Err(e);
        }
    };
    let mut child = match start_tray(&target) {
        Ok(c) => c,
        Err(e) => {
            let _ = fs::rename(&backup, &target);
            let _ = start_tray(&target);
            return Err(error(e));
        }
    };
    for _ in 0..100 {
        if request(&control, "ping\n").is_ok() {
            drop(lock);
            std::process::exit(0);
        }
        if child.exited() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.stop();
    let _ = fs::remove_file(&control);
    fs::rename(&backup, &target).map_err(error)?;
    let _ = start_tray(&target);
    Err("Новая версия не запустилась; восстановлена предыдущая версия".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_and_development_paths_are_not_self_updated() {
        let home = Path::new("/home/test");
        assert!(eligible_path(
            Path::new("/home/test/.local/bin/snapling"),
            home
        ));
        for p in [
            "/usr/bin/snapling",
            "/opt/snapling",
            "/home/test/target/release/snapling",
            "/home/test/.local/binary/snapling",
        ] {
            assert!(!eligible_path(Path::new(p), home));
        }
    }
    #[test]
    fn failed_replacement_preserves_original_and_success_keeps_rollback() {
        let dir = std::env::temp_dir().join(format!("snapling-update-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("snapling");
        let source = dir.join("new");
        fs::write(&target, b"old").unwrap();
        assert!(replace(&dir.join("missing"), &target).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"old");
        fs::write(&source, b"new").unwrap();
        let backup = replace(&source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(fs::read(backup).unwrap(), b"old");
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o755);
        assert!(verify_executable(&target).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
