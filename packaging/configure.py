"""
This script updates the cargo-bundle settings in launcher/Cargo.toml
and generates icons.

You shouldn't need this script unless you are creating production builds.

Requirements:
- `tomlkit` and `httpx` Python packages
- `imagemagick` program
"""

from pathlib import Path
import subprocess
from tempfile import NamedTemporaryFile
import httpx
import tomlkit
import sys

sys.path.append(str(Path(__file__).absolute().parent))

from utils import REPO_ROOT, get_env

ICON_PATH = REPO_ROOT / 'crates' / 'launcher' / 'assets' / 'icon.png'
WINDOWS_ICON_PATH = REPO_ROOT / 'crates' / 'launcher' / 'assets' / 'icon.ico'


def process_icons() -> bool:
    source_icon = get_env('LAUNCHER_ICON', '')
    if not source_icon:
        return False

    if source_icon.startswith('http://') or source_icon.startswith('https://'):
        with NamedTemporaryFile(delete=False) as temp_file:
            resp = httpx.get(source_icon, follow_redirects=True)
            resp.raise_for_status()
            temp_file.write(resp.content)
            source_icon = temp_file.name
    elif sys.platform == 'win32':
        source_icon = source_icon.replace('/', '\\')

    magick_cmd = 'magick'
    if sys.platform == 'linux':
        # github runner uses old ubuntu
        magick_cmd = 'convert'

    subprocess.check_call([magick_cmd, source_icon, '-resize', '512x512', ICON_PATH])
    subprocess.check_call(
        [magick_cmd, ICON_PATH, '-resize', '256x256', WINDOWS_ICON_PATH]
    )

    return True


def main():
    has_icon = process_icons()
    launcher_cargo_toml = REPO_ROOT / 'crates' / 'launcher' / 'Cargo.toml'
    parsed = tomlkit.parse(launcher_cargo_toml.read_text())
    bundle_metadata = parsed['package']['metadata']['bundle']['bin']['launcher']
    bundle_metadata['name'] = get_env('LAUNCHER_NAME')
    bundle_metadata['identifier'] = get_env('LAUNCHER_APP_ID')
    if has_icon:
        bundle_metadata['icon'] = [str(ICON_PATH)]
    launcher_cargo_toml.write_text(tomlkit.dumps(parsed))


if __name__ == '__main__':
    main()
