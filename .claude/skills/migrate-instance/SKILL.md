---
name: migrate-instance
description: Convert an instance from the old instance-builder spec.json (plus its source folder, uploaded to the old backend) into a potato pack repo, verify it builds, and publish it. Use when moving an existing Potato Launcher deployment to pack repos, or when asked to migrate a spec.json instance.
---

# Migrate a spec.json instance to a pack repo

Inputs to collect from the user before starting:

- the old `spec.json` and the instance `id` to migrate
- the Prism instance folder that matches the old source folder (the instance
  set up in Prism, not the copy uploaded to the backend)
- where the new pack repo should live, and its git remote if any
- the new site folder and publish target (see `docs/setting-up/hosting.md`)

Read `docs/creating-instances.md` first for the `potato.toml` format.

## Steps

1. `potato init --id <id> --prism <prism folder>` in the new repo folder.
2. Fill `potato.toml` from the old spec entry:

   | spec.json | potato.toml |
   |---|---|
   | `download_server_base` | `[publish] base_url` (must stay the URL launchers already use, usually `https://<domain>/data`) |
   | `replace_download_urls` | `[publish] mirror` |
   | `display_name`, `auth_backend`, `default_xmx` | same keys at the top level |
   | `minecraft_version`, `mod_loader`, `loader_version` | filled by `pull prism`; check they match the spec |
   | `resource_sync` | `[client] resource_sync` |
   | `mod_sync.mode` | `[client] mod_sync` (a plain string) |
   | `content_rules` | `[[paths]]`, same fields; drop rules for `mods` |
   | `source_root` | not needed |

   Set `[publish] target` to the new site's `data/` folder.
3. `potato pull prism`, then check `git diff --cached`. Every managed path
   should come over, and every mod should have an index file.
4. Port mod references. They used mod ids; `required` and optional sets now
   use index names. Run `potato mod list` to map mod ids to index names.
   - `mod_sync.required` → `[client] required` (index names)
   - `mod_sync.optional_sets[].mod_ids` → `[[client.optional_sets]] mods`
     (index names), keeping `id`, `display_name`, `enabled_by_default`
   - `mod_sync.blocked` → `[client] blocked`, unchanged (mod ids)
5. Port `os_specific_mods`. For each glob, find the jars it matched in the old
   source folder. The jar for the current OS came over with `pull prism`; set
   `os = [...]` in its index file. Add each other variant with
   `potato mod add <jar> --os <os>`. Old OS names: `osx` is `macos` now.
6. Compare against the old source folder: any jar or file there that isn't in
   the repo now is either missing from Prism or outside every `[[paths]]`
   entry. Ask the user which.
7. Commit, then `potato build`. Fix errors until it builds.
8. Compare `.potato/build/` metadata with the old published
   `data/instances/<id>/meta.json`: same mod ids, same content rule paths,
   same loader. Report differences to the user before publishing.
9. `potato publish`.

## Switching the site

Launchers read one `instance_manifest.json`. An instance published to the new
site is invisible to players until nginx serves the new site, and instances
not migrated yet disappear when it does. So:

1. Migrate and publish every instance into the new site folder.
2. Publish a launcher release (`v*` tag) so `launcher/` exists in the new site.
   Old launchers update through the nginx rewrites to these files.
3. Stop the old compose stack and start the new one on the same port.
4. Check in a launcher that every instance installs and starts.

## Old files to delete by hand

Once every instance is republished and players are on the new site, list
these for the user to delete. Don't delete anything yourself:

- the old compose stack's containers and images: `potato-launcher-backend`,
  `potato-launcher-frontend` (nginx), `filebrowser`
- the old `state/` folder:
  - `state/generated/`: the old published data, including
    `instances/<id>/minecraft/`, `libraries/`, `assets/`, `versions/`,
    `java_mirror/` and the old `instance_manifest.json`
  - `state/workdir/`: the old builder cache
  - `state/internal/`: the old `spec.json` and uploaded instance folders.
    Keep a copy of `spec.json` until the user confirms nothing is missing.
  - `state/launcher/`: launcher files in the old `<os>/<artifact>/` layout
  - `state/filebrowser-db/`
- the old `.env` with `ADMIN_JWT_SECRET` and `ADMIN_SECRET_TOKEN`
- GitHub Actions secrets and variables that are no longer used:
  `ADMIN_SECRET_TOKEN`, `BACKEND_API_BASE`, `PUSH_IMAGES`, `GPG_PUBLIC_KEY`
