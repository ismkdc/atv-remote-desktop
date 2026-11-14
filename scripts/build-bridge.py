"""Build the local Python helper for Tauri's target-suffixed sidecar convention."""
import os
import pathlib
import shutil
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent.parent
target = subprocess.check_output(['rustc', '-vV'], text=True).split('host: ')[1].splitlines()[0]
signing = ['--codesign-identity', os.environ['APPLE_SIGNING_IDENTITY']] if sys.platform == 'darwin' and os.environ.get('APPLE_SIGNING_IDENTITY') else []
subprocess.run([sys.executable, '-m', 'PyInstaller', '--noconfirm', '--onefile',
    *signing, '--name', 'atv-bridge', '--collect-all', 'pyatv', '--distpath', str(root / 'work/sidecar'),
    '--workpath', str(root / 'work/pyinstaller'), '--specpath', str(root / 'work'),
    str(root / 'Support/bridge.py')], cwd=root, check=True)
extension = '.exe' if sys.platform == 'win32' else ''
destination = root / 'src-tauri/binaries' / f'atv-bridge-{target}{extension}'
destination.parent.mkdir(parents=True, exist_ok=True)
shutil.copy2(root / 'work/sidecar' / f'atv-bridge{extension}', destination)
