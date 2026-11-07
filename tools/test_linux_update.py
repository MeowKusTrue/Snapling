"""Integration check on a systemd Linux desktop; uses only an isolated temp profile.

python tools/test_linux_update.py /absolute/path/to/snapling
Exercises service handoff, graceful restart and rollback without touching the user's install.
"""
import hashlib
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time

binary = Path(sys.argv[1]).resolve()
root = Path(tempfile.mkdtemp(prefix='snapling-update-integration-'))
env = os.environ.copy()
env.setdefault('DBUS_SESSION_BUS_ADDRESS', f'unix:path=/run/user/{os.geteuid()}/bus')
env.update(HOME=str(root / 'home'), XDG_CONFIG_HOME=str(root / 'config'),
           XDG_DATA_HOME=str(root / 'data'), XDG_STATE_HOME=str(root / 'state'),
           XDG_CACHE_HOME=str(root / 'cache'), XDG_RUNTIME_DIR=str(root / 'run'),
           XDG_CURRENT_DESKTOP='SnaplingUpdateTest')
for key in ('SNAPLING_UPDATE_WORKER', 'SNAPLING_UPDATE_CALLER_PID'):
    env.pop(key, None)
for key in ('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_STATE_HOME', 'XDG_CACHE_HOME', 'XDG_RUNTIME_DIR'):
    Path(env[key]).mkdir(parents=True, exist_ok=True)
    Path(env[key]).chmod(0o700)
config = Path(env['XDG_CONFIG_HOME']) / 'snapling'
config.mkdir()
(config / 'config.toml').write_text('[ui]\nwelcomed = true\ncheck_updates = false\nauto_update = false\n')
sentinel = Path(env['XDG_DATA_HOME']) / 'user-data-preserved'
sentinel.write_text('keep me')
target = Path(env['HOME']) / '.local/bin/snapling'
target.parent.mkdir(parents=True)
shutil.copyfile(binary, target)
target.chmod(0o755)
control = Path(env['XDG_RUNTIME_DIR']) / f'snapling-{os.geteuid()}/control.sock'
units = []

def ping():
    try:
        with socket.socket(socket.AF_UNIX) as conn:
            conn.settimeout(1)
            conn.connect(str(control))
            conn.sendall(b'ping\n')
            return int(conn.recv(80).strip())
    except (OSError, ValueError):
        return None

def wait_for(predicate, message, timeout=20):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        if predicate():
            return
        time.sleep(.1)
    raise AssertionError(message)

def service(args, label):
    name = f'snapling-integration-{label}-{time.time_ns()}'
    units.append(name)
    cmd = ['systemd-run', '--user', '--collect', '--unit', name]
    for key, value in env.items():
        if key in ['HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_STATE_HOME',
                   'XDG_CACHE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CURRENT_DESKTOP', 'DBUS_SESSION_BUS_ADDRESS']:
            cmd.append(f'--setenv={key}={value}')
    subprocess.run(cmd + ['--'] + list(map(str, args)), check=True, capture_output=True)
    return name

try:
    service([target], 'tray')
    wait_for(ping, 'isolated old tray failed to start')
    old_pid = ping()
    original_hash = hashlib.sha256(target.read_bytes()).hexdigest()
    staged = root / 'new-snapling'
    staged.write_bytes(binary.read_bytes() + b'\nSNAPLING_UPDATE_INTEGRATION\n')
    staged.chmod(0o700)
    new_hash = hashlib.sha256(staged.read_bytes()).hexdigest()
    service([target, '--apply-update', staged], 'client')
    wait_for(lambda: hashlib.sha256(target.read_bytes()).hexdigest() == new_hash and ping() not in [None, old_pid],
             'service update did not replace/restart tray')
    assert hashlib.sha256(target.with_name('.snapling-update-previous').read_bytes()).hexdigest() == original_hash
    assert sentinel.read_text() == 'keep me'
    print('PASS: managed-service handoff, new tray, atomic replacement, previous version, user data', flush=True)

    # A real ELF that answers --version but cannot run as a tray must be rolled back.
    source = root / 'bad.rs'
    source.write_text('fn main() { if std::env::args().any(|a| a == "--version") { println!("snapling 999.0.0"); } }')
    bad = root / 'bad-snapling'
    subprocess.run(['rustc', str(source), '-o', str(bad)], check=True)
    before_pid = ping()
    service([target, '--apply-update', bad], 'rollback-client')
    wait_for(lambda: target.with_name('.snapling-update-previous').exists() and
             hashlib.sha256(target.read_bytes()).hexdigest() != new_hash, 'bad version was never installed')
    wait_for(lambda: hashlib.sha256(target.read_bytes()).hexdigest() == new_hash and ping() not in [None, before_pid],
             'failed new tray did not roll back/restart', timeout=30)
    assert sentinel.read_text() == 'keep me'
    print('PASS: failed startup restores old binary and restarts old tray without data loss', flush=True)
finally:
    pid = ping()
    if pid:
        cgroup = Path(f'/proc/{pid}/cgroup').read_text()
        for line in cgroup.splitlines():
            unit = line.rsplit('/', 1)[-1]
            if unit.startswith('snapling-tray-') and unit.endswith('.service'):
                units.append(unit)
        try:
            with socket.socket(socket.AF_UNIX) as conn:
                conn.connect(str(control))
                conn.sendall(b'quit-for-update\n')
                conn.recv(80)
        except OSError:
            pass
    for unit in units:
        subprocess.run(['systemctl', '--user', 'stop', unit], capture_output=True)
    print('Fixture:', root, flush=True)
