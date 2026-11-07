#!/bin/sh
# Run from the extracted official Linux bundle. No root, compilation or network needed.
set -eu
exec python3 - "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)" <<'PY'
from pathlib import Path
import os, shutil, subprocess, sys, tempfile
if os.geteuid() == 0:
    raise SystemExit('Run this installer as your normal desktop user, without sudo.')
source = Path(sys.argv[1])
version = subprocess.run([str(source / 'snapling'), '--version'], check=True, capture_output=True, text=True).stdout
if not version.startswith('snapling '):
    raise SystemExit('Unexpected application binary')
home = Path.home()
binary = home / '.local/bin/snapling'
apps = Path(os.environ.get('XDG_DATA_HOME', home / '.local/share')) / 'applications'
icons = apps.parent / 'icons/hicolor/scalable/apps'
for folder in [binary.parent, apps, icons]:
    folder.mkdir(parents=True, exist_ok=True)
fd, temporary = tempfile.mkstemp(prefix='.snapling-install-', dir=binary.parent)
os.close(fd)
try:
    shutil.copyfile(source / 'snapling', temporary)
    os.chmod(temporary, 0o755)
    os.replace(temporary, binary)
finally:
    if os.path.exists(temporary):
        os.unlink(temporary)
shutil.copyfile(source / 'snapling.svg', icons / 'snapling.svg')
quoted = '"' + str(binary).replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%') + '"'
(apps / 'snapling.desktop').write_text('[Desktop Entry]\nType=Application\nName=Snapling\nComment=Screenshots, annotation, OCR and translation\nExec=' + quoted + '\nIcon=snapling\nCategories=Utility;Graphics;\nStartupNotify=false\n', encoding='utf-8')
print(version.strip())
print('Installed:', binary)
print('Launch Snapling from your application menu. An already running copy must be restarted.')
PY
