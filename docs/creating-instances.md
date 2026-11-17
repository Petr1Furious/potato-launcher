# Creating instances

**Terminology:** Potato Launcher calls modpacks **instances**. An instance can
also be plain vanilla.

Each instance is a git repo, a **pack repo**. It holds only the files clients
and servers share: mod index files, configs and scripts. Jars never go into
git. You set the instance up in [Prism Launcher](https://prismlauncher.org), and the
`potato` CLI copies changes between that instance and the repo, builds the
instance and publishes it to your [host](/setting-up/hosting).

Forks of Prism, such as [Fjord Launcher](https://github.com/unmojang/FjordLauncher),
work too: use the fork's instance folder as the `prism` target.

## Installing potato

Linux and macOS:

- Nix: `nix run github:Petr1Furious/potato-launcher#potato -- --help`
- Release binaries: `potato-<os>-<arch>` on the GitHub releases page
- Cargo: `cargo install --git https://github.com/Petr1Furious/potato-launcher potato`

`potato` needs `git`, and `rsync` plus `ssh` for `rsync://` targets.

## Quick start

```bash
mkdir minigames && cd minigames
potato init --id minigames --prism ~/.local/share/PrismLauncher/instances/minigames
# edit potato.toml: [publish] and [[paths]]
potato pull prism
git diff --cached
git commit -m "first version"
potato publish
```

`init` creates `potato.toml`, a `.gitignore` with `.potato/`, and
`.potato/local.toml` with the Prism path. It reads the Minecraft and loader
versions from Prism's `mmc-pack.json`.

## Repo layout

The repo mirrors the game folder, but only for managed paths:

```
potato.toml
.gitignore                  # .potato/
mods/.index/*.pw.toml
resourcepacks/.index/*.pw.toml
config/...
kubejs/...
```

`.potato/` is local to your machine and never committed.

## potato.toml

```toml
id = "minigames"
display_name = { en = "Minigames", ru = "Миниигры" }
minecraft = "1.21.1"          # updated by `potato pull prism`
loader = "neoforge"           # vanilla | fabric | forge | neoforge
loader_version = "21.1.77"    # updated by `potato pull prism`
default_xmx = "4G"
auth_backend = { type = "ely_by" }
order = 10                    # position in the launcher's list; unset goes last

[publish]
base_url = "https://mc.example.com/data"
target = "rsync://user@mc.example.com/srv/potato/site/data"
mirror = false

[client]
resource_sync = "on_update"   # on_update | always | always_fast
mod_sync = "delta"            # delta | mirror | mirror_fast
required = ["sodium"]
blocked = ["optifine", "modrinth:xaeros-minimap"]

[[client.optional_sets]]
id = "shaders"
display_name = { en = "Shaders" }
enabled_by_default = false
mods = ["iris"]

[[paths]]
path = "config"
type = "directory"
skip_if_dir_exists = true

[[paths]]
path = "kubejs"
type = "directory"
overwrite = true
delete_extra = true

[servers.main]
exclude = ["config/somemod.toml"]
```

### Top level

- **id** (required): must start with a letter, then `a-z`, `0-9`, `_`, `-`
  (max 64). Used in URLs and on players' disks, so don't change it.
- **display_name**: a string or a language map. Falls back to `id`.
- **minecraft** (required), **loader**, **loader_version**: set from Prism on
  every `pull prism`. Without `loader_version`, the latest (Fabric, NeoForge)
  or recommended (Forge) version is used.
- **default_xmx**: default `-Xmx`, e.g. `4G`.
- **auth_backend**: auth provider for this instance. Omit to allow any.
- **order**: sort key in the instance list. Ties and unset values sort by id.

### `[publish]`

- **base_url** (required): public URL of the site's `data/` folder.
- **target** (required): where to write that folder. See
  [publish targets](/setting-up/hosting#publish-targets).
- **mirror**: if `true`, libraries, assets, Java and Modrinth jars are stored
  on your host too, instead of players downloading them from Mojang, Modrinth
  and Azul. Use it when those are blocked or slow for your players. Default:
  `false`.

### `[client]`

- **resource_sync**: when the launcher checks the client jar, libraries and
  assets.
- **mod_sync**: how a player's `mods/` is reconciled with the pack. `delta`
  only touches mods the pack changed, so players can add their own. `mirror`
  removes everything not in the pack.
- **required**: index names of mods players can't remove.
- **blocked**: mods players can't add, as mod ids or `modrinth:<slug>`. Use
  `potato mod id <jar | modrinth:slug>` to find a mod id.
- **optional_sets**: groups of mods players can turn on or off in the
  launcher. `potato mod list` lists index names.

Index names are the index file names without `.pw.toml`. `potato mod list` lists
them with their mod ids.

### `[[paths]]`

The managed paths, and how the launcher applies each one on clients. Only
these paths are copied between the repo and targets. `mods/` is always
managed and doesn't need an entry.

Common fields:

- **path**: relative to the game folder.
- **type**: `file`, `directory` or `config_options`.
- **apply_on**: `update` (default, only when the pack changes) or `always`
  (every launch).
- **version**: optional. Once a player has applied a rule with this version,
  it's skipped until you raise it. Useful for one-time changes players may
  undo later.

`file`:

- **overwrite** (default `true`): replace the player's copy if it differs. If
  `false`, the file is only written when missing.

`directory`:

- **overwrite** (default `true`): replace files that differ.
- **delete_extra** (default `true`): delete files the pack doesn't have.
- **skip_if_dir_exists** (default `false`): only write the folder if the
  player doesn't have it yet, so players can change it.

`config_options` changes single keys in a config file instead of shipping it:

- **config_type**: `json`, `yaml`, `toml` or `properties`.
- **options**: a list of `{ key = ["section", "name"], value = ... }`.

Deeper paths win over shallower ones, so you can mark `config` as
`skip_if_dir_exists` and still force one file inside it with a `file` rule.

### `[servers.<name>]`

- **exclude**: shared files this server keeps its own version of. See
  [Servers](/servers).

## Mods and packs

Mods live in `mods/.index/` as packwiz-style `.pw.toml` files, one per mod.
Prism writes these when you install a mod through its browser.
Resource packs, shader packs and datapacks work the same way in
`resourcepacks/.index/`, `shaderpacks/.index/` and `datapacks/.index/`. Their
folder needs a `[[paths]]` entry so clients know how to apply them.

Every index file in the repo has a download URL and a sha1. `pull prism`
converts Prism's files when needed:

- a mod already in the repo with the same jar keeps the repo's file, so your
  edits to it (like `side`) are kept
- otherwise the file is copied with its hash replaced by sha1
- CurseForge mods (Prism writes them without a URL) and jars without an index
  file are uploaded to your host, and their index file points there. Jars
  found on Modrinth by hash point at Modrinth instead.

Extra fields potato reads:

- **side**: `client`, `server` or `both` (default). Clients get `client` and
  `both`, servers get `server` and `both`.
- **os**: list of `windows`, `macos`, `linux`. Unset means every OS.

### Platform-specific jars

Some mods ship a different jar per OS with the same file name and mod id. Use
one index file per variant, each with its own `os`. Prism only has the jar for
your OS, so add the others by hand:

```bash
potato mod add voxy-linux.jar --os linux
```

`pull prism` and `push prism` only touch entries for the OS they run on, so a
collaborator on another OS doesn't replace your variant with theirs.

### Disabled mods

Disabling a mod in Prism (`.jar.disabled`) is only your local play state; the
mod stays in the pack. This lets you test any combination of optional mods in
the source instance. To remove a mod, delete it in Prism.

Whether a mod is optional, and on by default, is set only by
`[[client.optional_sets]]`.

### Adding a mod without Prism

```bash
potato mod add <file or URL> [--side server] [--os linux] [--dir resourcepacks] [--name NAME]
```

The jar is looked up on Modrinth by hash. If it isn't there, a URL source
keeps its URL and a local file is uploaded to your host.

## Syncing with Prism

```bash
potato pull prism    # Prism -> repo
potato push prism    # repo -> Prism
potato status        # what pull and push would change, for every target
```

`init --prism` adds the Prism instance as a target named `prism`. To change it
later, or to see and remove targets:

```bash
potato target add prism <instance folder>
potato target list
potato target remove <name>
```

`target add` picks the kind from the folder: one with `mmc-pack.json` is a Prism
instance, anything else a server. `--prism` or `--server` sets it explicitly.

`pull` stages Prism's changes since the last sync in the repo, without
committing: changes made only in Prism apply, changes made only in the repo
stay, and changes made in both show up as normal git conflicts. Review with
`git diff --cached` and commit. If you drop the pulled changes (`git reset
--hard`), potato still counts them as synced: the next `pull` won't bring them
back and the next `push` removes them from Prism. To get them back, run
`potato pull prism --force`, which ignores the last sync and compares Prism
with your last commit.

You can pull again before committing; each pull adds on top of the last one.
Other uncommitted changes are fine too. git refuses a pull that would overwrite
unstaged edits to the same file. Note that `git reset --hard` to drop a pull
then drops your own uncommitted changes as well.

`push` writes the last commit into Prism. It refuses if Prism changed since
the last sync, if this Prism instance was never synced with this repo (the
first push on a new machine or from a fresh clone), or if pack files
(`potato.toml`, index files, managed paths) have uncommitted changes, since
those wouldn't be pushed. Other uncommitted files don't matter. `--force`
overwrites anyway. It keeps your enabled/disabled choice for each mod, and new
mods from an off-by-default optional set arrive disabled.

Each target's last sync is recorded in the repo's `.potato/synced/<name>`, so it
is local to your clone.

## Publishing

```bash
potato build      # build the last commit into .potato/build/, without uploading
potato publish
```

`publish` builds the last commit (not uncommitted changes) and uploads it. It
refuses to run unless:

- `master` is checked out
- pack files have no uncommitted changes
- the last publish of this instance is in your history, so you can't revert
  someone else's publish
- the instance id isn't published from a different repo

`--force` skips all but the first. To roll back, check out an older commit
and run `potato publish --force`.

Uploads list each pack file and jar. Libraries, assets, Java and game jars are
shown as one line per group; `-v` lists every file.
`potato unpublish` removes the instance from the instance list.

### Cleaning up storage

Published files are never deleted by `publish`. Run `potato gc` from any
instance repo to delete stored files that no published instance uses
anymore. Versions replaced in the last 7 days are kept for players still
downloading them (`--grace-days` to change it).

## Collaborating

Everyone works on the same repo through git:

The first time, clone the repo, add your Prism instance with
`potato target add prism <instance folder>`, and run
`potato push prism --force`. After that:

```bash
git pull
potato push prism   # get the latest pack into your Prism instance
# play, change things in Prism
potato pull prism
git commit && git push
potato publish
```

Publishing needs write access to the host (SSH or S3 keys).

## Cache

Downloads, builds and hashes are cached in your user cache folder (on Linux
`~/.cache/potato`), shared by all repos. Set `POTATO_CACHE_DIR` to move it, and
run `potato cache clean` to delete it.
