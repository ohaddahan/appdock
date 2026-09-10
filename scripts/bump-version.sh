#!/bin/sh
set -eu

if [ "$#" -ne 0 ]; then
  echo 'Usage: scripts/bump-version.sh' >&2
  exit 2
fi

cd "$(dirname "$0")/.."
python3 - <<'PY'
from pathlib import Path
import re

manifest_path = Path('Cargo.toml')
lock_path = Path('Cargo.lock')
manifest = manifest_path.read_text()
lock = lock_path.read_text()

package = re.search(r'(?ms)^\[package\]\s*\n(.*?)(?=^\[|\Z)', manifest)
if package is None:
    raise SystemExit('Cargo.toml has no [package] section')
version = re.search(r'(?m)^version = "(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?"$', package[1])
if version is None:
    raise SystemExit('Cargo.toml has no supported package version')
old = version[0].split('"')[1]
new = f'{version[1]}.{int(version[2]) + 1}.0'

lock_pattern = r'(?m)(^name = "appdock"\nversion = ")' + re.escape(old) + r'("$)'
updated_lock, count = re.subn(lock_pattern, lambda m: m[1] + new + m[2], lock)
if count != 1:
    raise SystemExit('Cargo.lock must contain exactly one appdock package matching Cargo.toml')

start = package.start(1) + version.start()
end = package.start(1) + version.end()
manifest_path.write_text(manifest[:start] + f'version = "{new}"' + manifest[end:])
lock_path.write_text(updated_lock)
print(f'AppDock: {old} -> {new}')
PY
