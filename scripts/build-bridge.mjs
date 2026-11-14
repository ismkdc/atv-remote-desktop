import { existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const localPython = process.platform === 'win32' ? '.venv/Scripts/python.exe' : '.venv/bin/python';
const python = process.env.ATV_PYTHON || (existsSync(new URL(`../${localPython}`, import.meta.url)) ? localPython : process.platform === 'win32' ? 'python' : 'python3');
const result = spawnSync(python, ['scripts/build-bridge.py'], { cwd: root, stdio: 'inherit' });
if (result.error) console.error(result.error.message);
process.exit(result.status ?? 1);
