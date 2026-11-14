"""Verify the frozen helper imports and answers IPC without installed Python packages."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parent.parent
target = subprocess.check_output(['rustc', '-vV'], text=True).split('host: ')[1].splitlines()[0]
extension = '.exe' if sys.platform == 'win32' else ''
helper = root / 'src-tauri/binaries' / f'atv-bridge-{target}{extension}'
with tempfile.TemporaryDirectory() as data:
    result = subprocess.run([str(helper), data], input='{"action":"health"}\n', text=True,
                            capture_output=True, check=True, timeout=45)
    response = json.loads(result.stdout)
    assert response == {'ok': True, 'discovery': 'Bonjour' if sys.platform == 'darwin' else 'mDNS'}, (response, result.stderr)
print('Bundled helper IPC passed.')
