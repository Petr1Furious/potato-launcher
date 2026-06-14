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
    with open('flatpakref.template') as f:
        flatpakref_template = f.read()

    backend_api_base = get_env('BACKEND_API_BASE', '')

    variables = {
        'app_name': get_env('LAUNCHER_NAME'),
        'instance_manifest_urls': get_env('INSTANCE_MANIFEST_URLS', ''),
        'flatpak_id': get_env('LAUNCHER_APP_ID'),
        'app_name_lower': get_env('LAUNCHER_NAME').lower().replace(' ', '_'),
        'app_description': get_env('LAUNCHER_DESCRIPTION', ''),
        'flatpak_keywords': get_env('FLATPAK_KEYWORDS', ''),
        'backend_api_base': backend_api_base,
        'version': os.getenv('VERSION', ''),
    }

    if gpg_public_key := get_env('GPG_PUBLIC_KEY', None):
        variables['gpg_key_line'] = f'GPGKey={gpg_public_key}'
    else:
        variables['gpg_key_line'] = ''

    for k, v in variables.items():
        rgx = re.compile(r'{{\s*' + k + r'\s*}}')
        manifest_template = rgx.sub(v, manifest_template)
        desktop_template = rgx.sub(v, desktop_template)
        flatpakref_template = rgx.sub(v, flatpakref_template)

    with open('manifest.yml', 'w') as f:
        f.write(manifest_template)
    with open(variables['flatpak_id'] + '.desktop', 'w') as f:
        f.write(desktop_template)
    if backend_api_base:
        with open(variables['flatpak_id'] + '.flatpakref', 'w') as f:
            f.write(flatpakref_template)


if __name__ == '__main__':
    main()
