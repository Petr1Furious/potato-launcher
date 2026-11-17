# Pack repos and the `potato` CLI

Replaces the Go backend, the admin frontend, filebrowser, `instance-builder`,
`scripts/remote-instance.py` and `scripts/merge_specs.py`. Implemented in
`crates/potato`; user docs are in `docs/creating-instances.md`,
`docs/servers.md` and `docs/setting-up/hosting.md`.

## Goals

- Edit where it's convenient: the client in Prism, the server in its own folder.
  No hand-written JSON spec.
- Git is the source of truth and the collaboration tool.
- The storage host only serves static files. Everything complex (Forge
  installer, hashing, metadata generation) runs on the machine that publishes.
- Jars are never stored in git.

## Non-goals (for now)

- CI publishing of instances. Publishing is local.
- A web admin UI.
- Migration tooling beyond the `migrate-instance` skill.
- Paper servers.

## Core idea

Each instance is one git repo holding only what clients and servers share: mod
and pack index files, configs and scripts. The repo is a separate folder, not
the Prism instance or the server folder. Potato copies files between the repo
and those folders, which are called **targets**:

| Target | What it is | `pull` (target → repo) | `push` (repo → target) |
|---|---|---|---|
| `prism` | your Prism instance, the source for the client | yes, including new files | yes, for collaborators |
| a server | a modded server's game folder | changes to files the repo already has | yes |

Everything server-only (`server.properties`, server-only mods and their
configs, permissions, world, ops, whitelist) stays on the server and never
enters the repo.

## Repo layout

The repo mirrors the game folder for the managed paths only:

```
potato.toml
.gitignore                      # .potato/
mods/.index/*.pw.toml
resourcepacks/.index/*.pw.toml  # also shaderpacks/, datapacks/
config/…
kubejs/…
```

### `potato.toml` (committed)

```toml
id = "minigames"
display_name = { en = "Minigames", ru = "Миниигры" }
minecraft = "1.21.1"          # updated by `pull prism` from mmc-pack.json
loader = "neoforge"           # vanilla | fabric | forge | neoforge
loader_version = "21.1.77"
default_xmx = "4G"
auth_backend = { type = "ely_by" }
order = 10                    # sort key in the instance list

[publish]
base_url = "https://mc.example.com/data"
target = "rsync://minecraft@example.com/srv/potato/site/data"  # or file:// or s3://
mirror = false                # true: serve libraries, assets, Java and Modrinth jars from our storage

[client]
resource_sync = "on_update"
mod_sync = "delta"
required = ["sodium"]                              # index names
blocked = ["optifine", "modrinth:xaeros-minimap"]  # mod ids or Modrinth slugs

[[client.optional_sets]]
id = "shaders"
display_name = { en = "Shaders" }
enabled_by_default = false
mods = ["iris"]               # index names

# Managed paths, with how the launcher syncs each one on clients.
[[paths]]
path = "config"
type = "directory"
skip_if_dir_exists = true

[servers.main]
exclude = ["config/somemod.toml"]   # shared files this server keeps its own version of
```

`mods/` is always managed. Other index folders (`resourcepacks/` and so on)
need a `[[paths]]` entry so clients know how to apply them; `build` fails
without one.

### `.potato/local.toml` (not committed)

Written by `potato target add`:

```toml
[targets.prism]
kind = "prism"
path = "/home/me/.local/share/PrismLauncher/instances/minigames"

[targets.main]
kind = "server"
path = "/srv/minecraft/minigames"
```

## Index files: packwiz `.pw.toml`

Prism writes `<dir>/.index/<name>.pw.toml` for every mod or pack installed
through its browser. We use that format as the lockfile: one file per mod, so
git merges rarely conflict. We only read and write these files ourselves; the
`packwiz` CLI isn't used, because its `index.toml` holds every file's hash and
would conflict on almost every merge.

Extra fields potato reads:

- `side = client | server | both` (missing means `both`). Clients get `client`
  and `both`; servers get `server` and `both`.
- `os = ["windows", "macos", "linux"]` (missing means all). Only for mods.

Every index file in the repo has a `[download] url` and a sha1 hash, so potato
only ever checks sha1. Prism's own files don't always look like that: it
writes sha512 for Modrinth mods, sha1 or md5 (older files: murmur2) for
CurseForge mods, and no URL for CurseForge mods.

Two index files are **equal when they describe the same jar** (same sha1 of
the jar), however they're written:

- `pull prism` keeps the repo's file if it describes the same jar as Prism's.
  Otherwise it writes a converted copy: sha1 instead of Prism's hash, and
  `<base_url>/objects/<ab>/<sha1>` as the URL if Prism's has none (the jar is
  uploaded there). When the jar changed, the repo's `side` and `os` are kept.
- `push prism` writes an index file only if Prism has none for that mod or its
  file describes a different jar.

Jar hashes are cached by path, size and modification time.

### OS variants

Mods with one jar per OS (same filename and mod id) get one index file per
variant, each with its own `os`. `pull prism` and `push prism` only touch
entries for the OS they run on. Other variants are added with
`potato mod add <jar|url> --os <os>`.

### Disabled mods

A `.jar.disabled` jar in Prism is still part of the pack: disabling is only
local play state. Optional sets and their defaults come only from
`potato.toml`. `push prism` keeps each mod's local enabled/disabled state, and
new mods in an off-by-default set arrive disabled.

## Syncing with targets

Each target remembers the repo commit it last matched, its **base**, in
`<target>/.potato/synced-commit`. A local ref `refs/potato/<target>` keeps it
from being garbage-collected. If the base commit isn't in the repo, the target
counts as never synced.

Both directions need a clean repo tree.

### `potato pull <target>`

1. Builds a **snapshot commit** on top of the base: the base tree with the
   managed paths replaced by what's in the target now.
2. Applies it to `HEAD` with `git cherry-pick --no-commit`, so the three-way
   base is the old base: changes made only in the target apply, changes made
   only in the repo stay, and changes made on both sides become normal git
   conflicts. Pending cherry-pick state is cleared (`--quit`), so the result is
   plain staged changes and your commit is an ordinary one-parent commit.
3. Sets the target's base to the snapshot. The snapshot never enters shared
   history; `refs/potato/<target>` keeps it from being garbage-collected.

Discarding a pull (`git reset --hard`) means the repo wins: the target still
matches its base, so the next `push` overwrites those changes.

What gets read:

- **`prism`:** everything under the managed paths, including new and deleted
  files, with index files compared by jar. It also reads `mmc-pack.json` into
  `potato.toml`, and uploads (under the storage lock) CurseForge jars without a
  URL and jars without an index file. Unindexed jars are looked up on Modrinth
  (`/version_file/{sha1}`) first.
- **A server:** only changes to and deletions of files the base already has,
  plus paths given with `--add`. New files are ignored, because they're almost
  always server-only mod configs. Excluded paths and jars are never read.

With no base yet (either kind, or a base this clone doesn't have), files are
compared against `HEAD`. The snapshot is a root commit, so differences become
additions or conflicts, and deletions don't apply, so they aren't listed.

### `potato push <target>`

Refuses if the target's managed files changed since the base ("pull first"),
or, for servers, while the world's `session.lock` is held. A target with no
base in this repo (never synced, or synced from another clone) needs `--force`
too. `--force` skips all of these checks. Otherwise it:

- writes `HEAD`'s managed files, minus that server's `exclude`
- for `prism`, also writes index files (skipped when Prism's describes the same
  jar); servers get jars only, and only for `mods/`
- deletes files and jars that were in the base but are gone from `HEAD`
- downloads missing jars for the target's side and OS
- records `HEAD` as the new base

It never touches anything the base didn't contain: server-only mods, their
configs, and jars you put there by hand.

On a server, potato runs on the server itself, against a clone of the repo.
Loader installation is left to the server runtime (itzg).

### `potato status`

The same comparisons as pull and push for every target, without changing
anything.

## Publishing

### Site layout

One folder is served as the site root:

```
index.html                         # downloads page
launcher/<os>/<file>, launcher/<os>/version.txt
data/instance_manifest.json        # URL baked into launchers, unchanged
data/instances/<id>/meta.json      # InstanceMetadata, unchanged struct
data/instances/<id>/publish.json   # source_commit, repo_root, order, published_at, objects
data/history/<id>/<timestamp>.json # replaced publish.json records
data/objects/<ab>/<sha1>           # every stored file
data/.potato-lock                  # S3 lock object
```

`publish.json` is separate because the launcher re-serializes `meta.json` and
compares its sha1 with the manifest entry; any extra field would make it
download `meta.json` again on every sync.

Every stored file (uploaded jars, configs, patched Forge libraries,
authlib-injector, and mirrored libraries, client jar, assets and Java when
`mirror = true`) lives in `objects/<ab>/<sha1>`, where `<ab>` is the first two
hex characters of the sha1. That's Mojang's asset layout, so
`resources_url_base` is `<base_url>/objects/` when mirroring.

### Storage targets

- `file:///path`: local folder, `flock` on a lock file.
- `rsync://user@host[:port]/path`: `rsync` and `ssh` subprocesses. The lock is
  a long-lived `ssh host flock -n <lock> cat`. Needs GNU `find` and `flock` on
  the host.
- `s3://bucket/prefix`: `object_store`, credentials from `AWS_*` or `~/.aws`.
  The lock is an object created with a conditional write.

Every command that writes to storage (`publish`, `unpublish`, `pull prism`,
`mod add`, `gc`) holds the lock for its whole run.

### `potato build`

Builds `HEAD` (not the working tree) into `.potato/build/`, for inspection.
It stages the managed files, downloads every jar into the shared cache and
checks its sha1, then runs `crates/generate` with potato's own mod entries
(`InstanceGenerator::generate_with_mod_entries`; the generator's `mods/` scan
can't handle OS variants with the same filename). A rewrite pass then points
every file under `base_url` at `objects/<ab>/<sha1>`, keeping upstream URLs for
index entries unless `mirror = true`.

### `potato publish`

Refuses unless the tree is clean, `master` is checked out, the last publish's
`source_commit` is an ancestor of `HEAD`, and the instance id isn't published
from another repo (`repo_root`, the repo's root commit). `--force` skips the
last two checks. It always rebuilds, then:

1. uploads objects not already in storage, and fails if a referenced object
   is missing (a CurseForge or custom jar that was never uploaded)
2. writes `meta.json`
3. moves the previous `publish.json` to `history/` and writes the new one
4. updates this instance's entry in `instance_manifest.json`, sorted by
   `order`, then id

Rolling back means checking out an older commit and running `publish --force`.

### `potato unpublish`

Removes this instance's entry from the instance list.

### `potato gc`

Keeps objects listed by a current `publish.json`, by a history record newer
than the grace period (default 7 days), or uploaded within the grace period
(so jars uploaded by `pull prism` survive until they're published). Deletes
everything else in `objects/` and history records older than the grace period.

### `potato publish-launcher`

`potato publish-launcher --target <site root> --name <name> --version <tag>
<files…>` picks each file's OS from its name (`<Name>.exe`, `<lower>`,
`<lower>.flatpak`, `<lower>_macos.tar.gz`, `<Name>.dmg`), writes
`launcher/<os>/<file>` and one `launcher/<os>/version.txt` per OS, and
regenerates `index.html`. CI runs it on `v*` tags.

## Other commands

- `potato init [--id] [--prism]`: `potato.toml` template, `.gitignore`,
  `.potato/local.toml`, versions from Prism.
- `potato target add <name> <folder> [--prism|--server]`, `target list`,
  `target remove <name>`. Names are free; `init --prism` names its target
  `prism`. Without a flag, a folder with `mmc-pack.json` is Prism and anything
  else a server; the output names the kind. `add` checks the folder (Prism: has
  a `minecraft/` folder; server: no `mmc-pack.json`) and stores an absolute
  path.
- `potato mod add <file|url> [--os] [--side] [--dir] [--name]`
- `potato mod list`: index name, mod id, side and OS of every entry.
- `potato mod id <jar | modrinth:slug>`
- `potato cache clean`. The cache is in the user cache dir, overridable with
  `POTATO_CACHE_DIR`.
- Global `-C/--repo <dir>`, and `-v` to list every uploaded file. Without it,
  uploads list pack files and jars by path and runtime files (libraries,
  assets, Java, game jars) as one line per group.

## Launcher self-update

Each launcher build uses exactly one source, set at build time:
`LAUNCHER_UPDATE_URL` (`<url>/<os>/version.txt` and `<url>/<os>/<file>`) or
`LAUNCHER_UPDATE_GITHUB=owner/repo` (latest release, tag as version). Releases
happen on `v*` tags; the version is the tag.

Launchers built before this change update through `/api/v1/launchers/...`.
The nginx config in `docker/nginx/templates/` maps those URLs, and the legacy
`/version_<os>.txt` paths, onto `launcher/`.

## Workflows

**Client change:**

```
potato pull prism
git diff --cached && git commit && git push
potato publish
```

**Collaborator picking up changes:** `git pull`, then `potato push prism`.

**Server update** (on the server): `git pull`, stop the server,
`potato push main`, start it. If shared files were edited on the server,
`push` refuses: `potato pull main`, commit, `git push`, push again.

**Server-only mod:** drop the jar and its config on the server, or
`potato mod add <jar> --side server`.

**A shared config the server needs its own version of:** add it to that
server's `exclude`.

## Compatibility

The published output stays in the format today's launchers read, at the same
`data/instance_manifest.json` URL, so players don't need a launcher update.

## Later

- CI publishing on push to `master`
