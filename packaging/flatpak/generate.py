import os
from pathlib import Path
import sys
import re

sys.path.append(str(Path(__file__).absolute().parent.parent))

from utils import get_env


def main():
    with open('manifest-template.yml') as f:
        manifest_template = f.read()
    with open('desktop.template') as f:
        desktop_template = f.read()

    variables = {
        'app_name': get_env('LAUNCHER_NAME'),
        'instance_manifest_urls': get_env('INSTANCE_MANIFEST_URLS', ''),
        'flatpak_id': get_env('LAUNCHER_APP_ID'),
        'app_name_lower': get_env('LAUNCHER_NAME').lower().replace(' ', '_'),
        'app_description': get_env('LAUNCHER_DESCRIPTION', ''),
        'flatpak_keywords': get_env('FLATPAK_KEYWORDS', ''),
        'launcher_update_url': get_env('LAUNCHER_UPDATE_URL', ''),
        'launcher_update_github': get_env('LAUNCHER_UPDATE_GITHUB', ''),
        'launcher_data_dir': get_env('LAUNCHER_DATA_DIR', ''),
        'version': os.getenv('VERSION', ''),
    }

    for k, v in variables.items():
        rgx = re.compile(r'{{\s*' + k + r'\s*}}')
        manifest_template = rgx.sub(v, manifest_template)
        desktop_template = rgx.sub(v, desktop_template)

    with open('manifest.yml', 'w') as f:
        f.write(manifest_template)
    with open(variables['flatpak_id'] + '.desktop', 'w') as f:
        f.write(desktop_template)


if __name__ == '__main__':
    main()
