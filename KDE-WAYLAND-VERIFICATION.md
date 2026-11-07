# KDE Plasma / Wayland

The integration follows KDE's [KGlobalAccel D-Bus API](https://invent.kde.org/frameworks/kglobalaccel/-/blob/master/src/org.kde.KGlobalAccel.xml)
and [component signals](https://invent.kde.org/frameworks/kglobalaccel/-/blob/master/src/org.kde.kglobalaccel.Component.xml).
Capture options follow [Spectacle's CLI definitions](https://invent.kde.org/graphics/spectacle/-/blob/master/src/CommandLineOptions.h).

Requires `spectacle`, `wl-clipboard`, a working session D-Bus and KDE's `kglobalacceld`.
`XDG_CURRENT_DESKTOP` must contain the `KDE` token (case insensitive, colon-separated
lists are supported). Hyprland detection takes precedence.

Snapling uses Spectacle in a separate background instance with a private temporary
directory. `--screen` captures the entire desktop, `--window` the active window.
`--region`, `--delay`, `--ocr`, and `--ocr-translate` use Spectacle's native region
selection. KWin/Spectacle handle monitor positions and fractional scaling; the
composite image is never displayed across an unrelated single-monitor overlay.
The result goes through Snapling's clipboard, saving, history and configured editor.
Enable **Open editor** in capture settings for annotations after selection, or open
a saved image with `snapling --edit /absolute/path/image.png`.
Snapling's overlay toolbar, window hover targets, loupe and picker are not used in
this KDE path. Cross-monitor image composition follows the installed Spectacle version.

Start `snapling` without flags to register global shortcuts through KDE's native
KGlobalAccel D-Bus API. No GlobalShortcuts portal or portal permission dialog is used.
The Settings → Hotkeys page accepts text such as `Ctrl+Shift+S`; leave the field to
save. The tray reloads changes within about 500 ms, removes the old set, checks conflicts,
and compares the daemon's returned bindings with the requested keys. Failed bindings
are reported through notifications and stderr. Empty values disable actions. Invalid
TOML leaves the previous valid set active and reports the parse error.

PrintScreen and Shift+PrintScreen commonly belong to Spectacle already. Snapling never
steals another component's keys: choose a free combination or explicitly free it in
KDE System Settings. Letters, digits, F1–F24, standard navigation and punctuation keys
are supported; Windows virtual codes, mouse and multimedia/numpad keys currently are not.
Repeat region, recording and long screenshot actions are explicitly unsupported in KDE.
Other desktop environments retain their existing compositor/WM bindings.

Only one KDE tray instance owns the registrations. Tray exit, SIGINT and SIGTERM
remove them; a new tray clears stale registrations after a previous crash/SIGKILL.
KGlobalAccel restart triggers reconnect and registration. External reassignment or
revocation is reported without silently overwriting it; change the Snapling config
to register again. After SIGKILL, keys may remain reserved until Snapling restarts.

`--doctor` checks Spectacle's CLI, the KWin ScreenShot2 interface and KGlobalAccel,
and reports the daemon's actual bindings when a tray instance is running. These
checks do not prove capture authorization or physical key activation. Preflight
does not require shortcuts for direct CLI capture. On other generic Wayland sessions,
install `wayland-utils`: preflight uses `wayland-info` to verify the wlroots screencopy
protocol, rather than relying on the presence of `grim` alone.

Recording and scrolling screenshots still require wlroots protocols. KDE rejects
`--record` and `--long` with a clear explanation, even if those tools are installed.

### Verification on Linux

Run from the repository root (no release/tag/version change is needed):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# Mock KGlobalAccel in a private bus, never in the desktop's existing bus:
dbus-run-session -- cargo test -p snapling-unix private_bus -- --ignored --nocapture
cargo build --release -p snapling-app
```

For manual tests, first exit the old installed tray. Use isolated data and a writable
test picture directory; these environment variables also apply to child actions:

```sh
mkdir -p target/kde-test/{config,data,state,cache,pictures}
export XDG_CONFIG_HOME="$PWD/target/kde-test/config"
export XDG_DATA_HOME="$PWD/target/kde-test/data"
export XDG_STATE_HOME="$PWD/target/kde-test/state"
export XDG_CACHE_HOME="$PWD/target/kde-test/cache"
export XDG_PICTURES_DIR="$PWD/target/kde-test/pictures"
printf 'XDG_PICTURES_DIR="%s"\n' "$PWD/target/kde-test/pictures" > "$XDG_CONFIG_HOME/user-dirs.dirs"
./target/release/snapling --settings
./target/release/snapling --doctor
./target/release/snapling --screen
./target/release/snapling --window
./target/release/snapling --region
# Escape in the selector: expected exit code 5 (no image, no stderr).
echo "$?"
./target/release/snapling > target/kde-test/tray.log 2>&1 &
snapling_pid=$!
./target/release/snapling --settings
./target/release/snapling --doctor --json
# After finishing the checks:
kill -TERM "$snapling_pid"
wait "$snapling_pid"
```

Check clipboard paste and saved PNGs, open the editor, save annotated images, and select
areas on each monitor and across their boundary. Include 100%/150%/200% scale and a
monitor left/above the primary one. Check Escape and capture failure. Spectacle may
exit successfully on failure; if no region image exists, stderr is reported as an error,
and silent completion is treated as cancellation.

Set a free shortcut, physically activate it, change it and confirm the old key no longer
activates Snapling. Disable it with an empty field. Test a duplicate and a KDE-owned
shortcut; confirm the conflict message and that the other app retains its shortcut.
Start a second tray (must be refused), exit/restart the first, and verify registration
again. Revoke/reassign a key in KDE and confirm the notification; change it in Snapling
to register again. Check that tray exit/SIGTERM removes the keys. A private-bus mock
tests wire types, replacement, disabling, conflicts and AccessDenied, not real KWin.

On Hyprland, repeat `--screen`, region/overlay, recording, scrolling and configured
compositor binds. On X11, repeat capture, overlay, clipboard, saving and WM binds.
The Windows checks in AGENTS.md remain applicable.
