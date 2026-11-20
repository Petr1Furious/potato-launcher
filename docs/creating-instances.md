# Creating instances

**Note on terminology:** Potato Launcher calls these **instances**. In most cases instances will be modpacks, but it is also possible to deploy vanilla versions too.

Each instance lives in its own git repository, which we will call a **pack repo**. It holds the files clients and servers share: mod index files, configs, scripts, and jars that only you host. You set the instance up in [Prism Launcher](https://prismlauncher.org), and the `potato` CLI copies changes between that instance and the repo, builds the instance and publishes it to your [host](/setting-up/hosting).

::: info Prism forks
Forks of Prism, such as [Fjord Launcher](https://github.com/unmojang/FjordLauncher), work too. Use the fork's instance folder wherever these docs say Prism instance.
:::

## Installing potato

`potato` runs on Linux and macOS. You can install it in one of these ways:

- **Release binaries**: download `potato-<os>-<arch>` from the GitHub releases page
- **Cargo**: `cargo install --git https://github.com/Petr1Furious/potato-launcher potato`
- **Nix**: `nix run github:Petr1Furious/potato-launcher#potato -- --help`

`potato` needs `git`. For `rsync://` targets, you'll also need `rsync` and `ssh`, and for repos that use Git LFS, `git-lfs`.

## Quick start

1. Create the repo from your Prism instance:

```bash
mkdir minigames && cd minigames
potato init --id minigames --prism ~/.local/share/PrismLauncher/instances/minigames
```

This creates `potato.toml`, `.gitignore` with `.potato/`, and `.potato/local.toml` with the Prism instance as a target named `prism`. The Minecraft and loader versions are read from Prism's `mmc-pack.json`.

2. Edit `potato.toml`: set `[publish]` and add `[[paths]]` for the folders you want to sync. See [potato.toml](#potato-toml) below.

3. Pull the instance into the repo, review the changes and commit them:

```bash
potato pull prism
git diff --cached
git commit -m "first version"
```

4. Publish it:

```bash
potato publish
```

## Repo layout

The repo mirrors the game folder, but only for the managed paths:

```
potato.toml
.gitignore # .potato/
mods/.index/*.pw.toml
mods/*.jar # jars only you host
resourcepacks/.index/*.pw.toml
config/...
kubejs/...
```

`.potato/` is local to your machine and is never committed.

## potato.toml

```toml
id = "minigames"
display_name = { en = "Minigames", ru = "Миниигры" }
minecraft = "1.21.1"       # updated by `potato pull prism`
loader = "neoforge"        # vanilla | fabric | forge | neoforge
loader_version = "21.1.77" # updated by `potato pull prism`
default_xmx = "4G"
auth_backend = { type = "ely_by" }
order = 10                 # position in the launcher's list (unset = last)

[publish]
base_url = "https://mc.example.com/data"
target = "rsync://user@mc.example.com/srv/potato/site/data"
mirror = false

[client]
resource_sync = "on_update" # on_update | always | always_fast
mod_sync = "delta"          # delta | mirror | mirror_fast
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

- **id** (required): Must start with a letter, followed by `a-z`, `0-9`, `_` or `-` (max 64 characters). It's used in URLs and on players' disks, so don't change it later.
- **display_name**: A string or a language map. Default: `id`
- **minecraft** (required), **loader**, **loader_version**: Set from Prism on every `pull prism`. Without `loader_version`, the latest (Fabric, NeoForge) or recommended (Forge) version is used.
- **default_xmx**: Default `-Xmx`, for example `4G`
- **auth_backend**: Auth provider for this instance. Omit it to allow any.
- **order**: Sort key in the instance list. Ties and unset values are sorted by id.

### `[publish]`

- **base_url** (required): Public URL of the site's `data/` folder.
- **target** (required): Where to write that folder. See [publish targets](/setting-up/hosting#publish-targets).
- **mirror**: If `true`, libraries, assets, Java and Modrinth jars are stored on your host too, instead of players downloading them from Mojang, Modrinth and Azul. Use it when those are blocked or slow for your players. Default: `false`

### `[client]`

- **resource_sync**: When the launcher checks the client jar, libraries and assets.
- **mod_sync**: How a player's `mods/` folder is reconciled with the pack. `delta` only touches the mods the pack changed, so players can add their own. `mirror` removes everything that isn't in the pack.
- **required**: Index names of mods players can't remove.
- **blocked**: Mods players can't add, as mod ids or `modrinth:<slug>`. Use `potato mod id <jar | modrinth:slug>` to find a mod id.
- **optional_sets**: Groups of mods players can turn on or off in the launcher.

Index names are the index file names without `.pw.toml`, or the jar name without `.jar` for a committed jar. `potato mod list` lists them with their mod ids.

### `[[paths]]`

The managed paths, and how the launcher applies each of them on clients. Only these paths are copied between the repo and its targets. `mods/` is always managed and doesn't need an entry.

Common fields:

- **path**: Relative to the game folder.
- **type**: `file`, `directory` or `config_options`
- **apply_on**: `update` (only when the pack changes) or `always` (every launch). Default: `update`
- **version**: Once a player has applied a rule with this version, it's skipped until you raise it. Useful for one-time changes players may undo later.

`file`:

- **overwrite**: Replace the player's copy if it differs. If `false`, the file is only written when it's missing. Default: `true`

`directory`:

- **overwrite**: Replace files that differ. Default: `true`
- **delete_extra**: Delete files the pack doesn't have. Default: `true`
- **skip_if_dir_exists**: Only write the folder if the player doesn't have it yet, so players can change it. Default: `false`

`config_options` changes single keys in a config file instead of shipping the whole file:

- **config_type**: `json`, `yaml`, `toml` or `properties`. `properties` keeps each file's `=` or `:` separator, so it also works for `options.txt`.
- **options**: A list of `{ key = ["section", "name"], value = ... }`

Deeper paths win over shallower ones. For example, you can mark `config` as `skip_if_dir_exists` and still force one file inside it with a `file` rule.

### `[servers.<name>]`

- **exclude**: Shared files this server keeps its own version of. See [Servers](/servers).

## Mods and packs

A mod is either an index file in `mods/.index/` or a jar committed in `mods/`. Index files are packwiz-style `.pw.toml` files that say where to download the jar. Prism writes them when you install a mod through its browser.

Resource packs, shader packs and datapacks work the same way in `resourcepacks/`, `shaderpacks/` and `datapacks/`, but only once their folder has a `[[paths]]` entry. Without one, potato ignores the folder.

When you run `pull prism`, each jar in Prism ends up in the repo like this:

- If the repo already has the same jar, the repo's files stay as they are, including your edits to them (like `side`).
- If Prism knows a download URL for it, it becomes an index file, with its hash converted to sha1.
- Otherwise, potato looks it up on Modrinth by its hash. If Modrinth has it, the index file points at Modrinth.
- Everything else, like CurseForge mods (Prism writes them without a URL) and your own jars, is committed in `mods/`. If Prism has an index file for it, that file is kept next to the jar without its download section.

Index files can have these extra fields:

- **side**: `client`, `server` or `both`. Clients get `client` and `both`, servers get `server` and `both`. Default: `both`
- **os**: A list of `windows`, `macos` and `linux`. Only for mods. Default: every OS

To change them, use `potato mod set`:

```bash
potato mod set <name> [--os <os>]... [--side <side>]
```

`--os all` makes a mod apply to every OS again. For packs, use `resourcepacks/<name>` and so on as the name.

### Committed jars

A jar or zip committed directly in a pack folder is part of the pack, like a config file, and `push` and `publish` will use it.

To set its side or OS, run `potato mod set` with the jar's name. This writes an index file without a download URL next to it:

```bash
potato mod set mymod --side client
```

Committed jars make the repository grow with every update. To avoid that, store them with Git LFS:

```bash
git lfs install
git lfs track "mods/*.jar"
git add .gitattributes
```

::: warning
GitHub's free LFS quota is small, so packs with many CurseForge jars may need a different LFS host.
:::

### OS-specific jars

Some mods ship a different jar for each OS with the same mod id. Use one index file per variant, each with its own `os`. Prism only has the jar for your OS, so mark that one and add the others:

```bash
potato mod set voxy --os macos
potato mod add <windows jar or URL> --os windows --name voxy-windows
```

`pull prism` and `push prism` only touch entries for the OS they run on, so a collaborator on another OS doesn't replace your variant with theirs.

### Updating mods

When Prism updates a mod, the new jar usually has a different file name. `pull` treats it as an update of the mod whose jar disappeared if they share a Modrinth or CurseForge project, an index name, or (for mods) the mod id inside the jar. The update stays in the same index file and keeps its `side` and `os`.

If you update a mod in the repo, `push` keeps it disabled in Prism when the old version was disabled. If the repo and Prism were both updated to the same jar, `pull` doesn't produce a conflict.

### Disabled mods

Disabling a mod in Prism (`.jar.disabled`) only affects your local game, and the mod stays in the pack. This lets you test any combination of optional mods in your instance. To remove a mod from the pack, delete it in Prism.

Whether a mod is optional, and whether it's on by default, is set only by `[[client.optional_sets]]`.

### Adding a mod without Prism

```bash
potato mod add <file or URL> [--side server] [--os linux] [--dir resourcepacks] [--name NAME]
```

The file is looked up on Modrinth by its hash. If Modrinth doesn't have it, a URL source keeps its URL, and a local file is committed in its folder. For a committed file, an index file is only written if you pass `--side` or `--os`.

## Syncing with Prism

```bash
potato pull prism    # Prism -> repo
potato push prism    # repo -> Prism
potato status        # what pull and push would change, for every target
```

`init --prism` adds the Prism instance as a target named `prism`. To change it later, or to see and remove targets:

```bash
potato target add prism <instance folder>
potato target list
potato target remove <name>
```

`target add` picks the kind of target from the folder: a folder with `mmc-pack.json` is a Prism instance, anything else is a server. Pass `--prism` or `--server` to set it yourself.

### Pull

`pull` stages the changes made in Prism since the last sync, without committing them. Changes made only in Prism are applied, changes made only in the repo stay, and changes made on both sides show up as normal git conflicts. Review them with `git diff --cached` and commit.

The last sync is your last push, or your last commit that matches a pull exactly.

You can pull several times before committing, and each pull adds to the previous one. Other uncommitted changes in the repo are fine too, but git refuses a pull that would overwrite unstaged edits to the same file.

If you drop the pulled changes with `git reset --hard`, the next `pull` brings them back. To remove them from Prism instead, run `potato push prism --force`.

::: warning
`git reset --hard` also drops your own uncommitted changes.
:::

`potato pull prism --force` makes the repo match Prism exactly: it stages files in `mods` and the `[[paths]]` files and folders as they are in Prism, including deletions.

### Push

`push` writes the last commit into Prism. It keeps your enabled/disabled choice for each mod, and new mods from an off-by-default optional set arrive disabled. It refuses to run if:

- Prism has changes you haven't committed. Pull them first.
- Pack files (`potato.toml`, index files, committed jars, managed paths) have uncommitted changes, since those wouldn't be pushed.

`potato push prism --force` skips these checks and makes Prism match the last commit exactly: in `mods` and the `[[paths]]` folders, everything the repo doesn't have is deleted, including disabled jars.

Push soon after changing the repo, so later pulls have fewer conflicts.

## Publishing

```bash
potato build      # build the last commit into .potato/build/, without uploading
potato publish
```

`publish` builds the last commit (not your uncommitted changes) and uploads it. It refuses to run if:

- Pack files have uncommitted changes.
- The last publish of this instance isn't in your history. This stops you from reverting someone else's publish.
- The instance id is published from a different repo.

`--force` skips these checks. You can publish from any branch, but after you do, a publish from a branch that doesn't contain yours refuses until it's merged. To roll back, check out an older commit and run `potato publish --force`.

While uploading, `publish` lists every pack file and jar, and shows libraries, assets, Java and game jars as one line per group. Pass `-v` to list every file.

To remove the instance from the instance list, run `potato unpublish`.

### Cleaning up storage

`publish` never deletes files. To delete stored files that no published instance uses anymore, run `potato gc` from any instance repo.

The files of a replaced or unpublished version are kept for 2 hours, for launchers that are still on it; use `--grace-hours` to change it.

## Collaborating

Everyone works on the same repo through git.

The first time, you'll need to clone the repo, add your Prism instance and push the pack into it:

```bash
potato target add prism <instance folder>
potato push prism
```

If the instance has files that differ from the pack, push refuses and lists them.

After that, the usual workflow is:

```bash
git pull
potato push prism # get the latest pack into your Prism instance
# play and change things in Prism
potato pull prism
git commit && git push
potato publish
```

Publishing needs write access to the host (SSH or S3 keys).

## Cache

Downloads, builds and hashes are cached in your user cache folder (`~/.cache/potato` on Linux, `~/Library/Caches/potato` on macOS) and shared by all repos. Set `POTATO_CACHE_DIR` to move it, and run `potato cache clean` to delete it.
