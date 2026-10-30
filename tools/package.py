"""Release build, then target/dist: Snapling-Setup.exe and Snapling-portable.zip."""
import os
import subprocess
import zipfile

ISCC = r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(ROOT)
subprocess.run(["cargo", "build", "--release"], check=True)
subprocess.run([ISCC, "/Q", r"installer\snapling.iss"], check=True)
with zipfile.ZipFile(r"target\dist\Snapling-portable.zip", "w", zipfile.ZIP_DEFLATED) as z:
    z.write(r"target\release\snapling.exe", r"Snapling\snapling.exe")
    z.write("LICENSE", r"Snapling\LICENSE.txt")
    z.write("THIRD_PARTY_NOTICES.md", r"Snapling\THIRD_PARTY_NOTICES.md")
    z.writestr(r"Snapling\portable.txt", "Settings and history live in the data folder next to the exe.\n")
for name in os.listdir(r"target\dist"):
    print(name, os.path.getsize(os.path.join(r"target\dist", name)) // 1024 // 1024, "MB")
