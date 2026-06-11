# Creating instances

**Note on terminology:** Potato Launcher calls these **instances**. In most cases instances will be modpacks, but technically you can deploy vanilla versions too.

## Recommended: use the Web UI

If you deployed the full server setup (see [Server setup](/setting-up/server)), you can manage instances from the browser. Typical workflow:

- Login into the admin panel at `https://<your-domain>/admin`
- Click **New Instance** or select an instance you want to update and click **Update**
- Fill all necessary fields (**Instance Name**, **Minecraft Version**, **Mod Loader**, **Loader version** (if not vanilla), **Authentication Type**, **Mod Sync**, and **Resource Sync**)
- Click **Create Instance**
- To upload modpack files (e.g. mods, configs), select an existing instance, click **Update** and **Manage instance files**. Upload all files you want the builder to include.
- **Important:** add **content rules** and choose a **type** for each rule (`file`, `directory`, or `config_options`). The launcher needs these rules to know how to sync each path when the remote pack changes. For example, set **Skip if dir exists** on `config`, and both **Overwrite** and **Delete Extra** on `kubejs`.
- Click **Build** to generate the `/data/` output served by nginx

The launcher downloads metadata from `<download_server_base>/instance_manifest.json` (usually `https://<your-domain>/data/instance_manifest.json`).

## Manual

Linux/macOS is recommended for building instances. However, Windows should also work.

If you don't have Rust installed, get it from [rustup.rs](https://rustup.rs).

Clone the repository and build from the workspace root:

```bash
git clone <your-repository-url>
cd <repository-name>
```

Then, you'll need to create a `spec.json` file. It's used to define launcher instances that should be created. The file format is described below. You can also find an example config at [`crates/instance-builder/spec.example.json`](https://github.com/Petr1Furious/potato-launcher/blob/master/crates/instance-builder/spec.example.json)

After defining your instance, you can build it with the following command:

```bash
cargo run --release -p instance-builder -- -s <path to spec.json>
```

This will create a `generated` directory, which should then be uploaded to your server. If you followed the [Server configuration](/setting-up/server) guide, you should upload the contents of this directory (not the directory itself) to the `data` subdirectory of your launcher dir, e.g. to `/srv/potatosmp/data`.

## Manual (remote server build via SSH)

If you already have the backend deployed and you want to automate uploading files and building instances, you can:

- Pick the backend **internal directory** on the server, e.g. `/srv/potato-launcher/state/internal`
- Upload your `spec.json` into `<internal-dir>/spec.json`
- Upload your raw modpack files into `<internal-dir>/uploaded-instances/<instance-name>/`
- Run `instance-builder` inside the running backend container (`potato-launcher-backend` by default)

This repository includes a helper script that automates the above:

```bash
python3 scripts/remote-instance.py build --help
python3 scripts/remote-instance.py fetch --help
```

The build command uploads a temporary copy of `spec.json` with `source_root` rewritten to the in-container uploaded instance path (default: `/data/internal/uploaded-instances/<instance-name>`). It does not modify your local spec file. Instance sync skips `.git` and `saves` by default; see `--help` for flags to change that.

You can keep common settings in `scripts/remote-instance.json`:

```json
{
  "remote": "minecraft@example.com",
  "ssh_port": 22,
  "internal_dir": "/srv/potato-launcher/state/internal",
  "container": "potato-launcher-backend",
  "docker_host": "unix:///run/user/1002/docker.sock",
  "spec": "./spec.json",
  "instances": {
    "Minigames": "/local/path/to/instance/minecraft"
  }
}
```

## JSON structure

```json
{
  "download_server_base": "string",
  "replace_download_urls": "boolean",
  "instances": [
    {
      "name": "string",
      "minecraft_version": "string",
      "mod_loader": "string",
      "loader_version": "string",
      "source_root": "string",
      "content_rules": [
        {
          "path": "string",
          "apply_on": "update | always",
          "type": "file",
          "overwrite": "boolean"
        },
        {
          "path": "string",
          "apply_on": "update | always",
          "type": "directory",
          "overwrite": "boolean",
          "delete_extra": "boolean",
          "skip_if_dir_exists": "boolean"
        },
        {
          "path": "string",
          "apply_on": "update | always",
          "type": "config_options",
          "config_type": "json | yaml | toml | properties",
          "options": []
        }
      ],
      "mod_sync": {
        "mode": "delta | mirror | mirror_fast",
        "required": ["string (mod-id)"],
        "blocked": ["string (mod-id)"],
        "optional_sets": []
      },
      "resource_sync": "on_update | always | always_fast",
      "auth_backend": {
        "type": "string"
      },
      "default_xmx": "string"
    }
  ]
}
```

## Root fields

- **download_server_base** (required): Base URL where generated files are deployed. Files must be reachable at `<download_server_base>/<relative-path>`. Typically `https://your.domain/data`.
- **replace_download_urls**: If `true`, libraries, assets, and included files are served from your server. If `false`, Mojang/upstream URLs are kept where possible; only metadata, content rules, and Forge patched jars come from your server. Default: `false`.
- **instances** (required): Array of instance specs (see below).

## Instance fields

- **name** (required): Instance name.
- **minecraft_version** (required): Minecraft version.
- **mod_loader**: `vanilla`, `fabric`, `forge`, or `neoforge`. Default: `vanilla`.
- **loader_version**: Mod loader version. Optional; if omitted, Fabric/Forge/NeoForge generators pick a default (latest / recommended / latest).
- **source_root**: Directory containing authored pack files. Required when `content_rules` is non-empty. Web UI / backend set this to the uploaded instance directory on build.
- **content_rules**: Rules for syncing authored files into the client instance directory.
- **mod_sync**: How local `mods/` is reconciled with the remote mod list.
- **resource_sync**: When to verify client jar, libraries, and assets.
- **auth_backend**: Auth provider for this instance. Omit to allow any provider.
- **default_xmx**: Default JVM `-Xmx` (e.g. `4G`, `8192M`).

## Content rules

Each rule is a tagged object. Shared fields:

| Field      | Applies to | Default  | Description                                                   |
| ---------- | ---------- | -------- | ------------------------------------------------------------- |
| `path`     | all        | —        | Path relative to `source_root`                                |
| `type`     | all        | —        | `file`, `directory`, or `config_options`                      |
| `apply_on` | all        | `update` | `update` = only on instance update; `always` = on each launch |

### `type: "file"`

Sync a single file. The builder hashes the file and records download metadata.

| Field       | Default | Description                                   |
| ----------- | ------- | --------------------------------------------- |
| `overwrite` | `true`  | Re-check/download even if file exists locally |

### `type: "directory"`

Sync a directory tree.

| Field                | Default | Description                                                    |
| -------------------- | ------- | -------------------------------------------------------------- |
| `overwrite`          | `true`  | Re-check/download per-file even if it exists locally           |
| `delete_extra`       | `true`  | Delete local directory files not in the remote manifest        |
| `skip_if_dir_exists` | `false` | Skip rule if directory exists with `.download_complete` marker |

`overwrite: true` still forces per-file re-check regardless of `skip_if_dir_exists`.

### `type: "config_options"`

Patch config options in an existing file.

| Field         | Description                                     |
| ------------- | ----------------------------------------------- |
| `config_type` | `json`, `yaml`, `toml`, or `properties`         |
| `options`     | Array of `{ "key": ..., "value": ... }` entries |

- `key` may be a string (`"difficulty"`) or path array (`["mods", 0, "enabled"]`) for nested access.
- `properties` supports flat string keys only.
- `value` is any JSON value.

Example:

```json
{
  "path": "config/zoomify.json",
  "apply_on": "update",
  "type": "config_options",
  "config_type": "json",
  "options": [{ "key": ["initialZoom"], "value": 4 }]
}
```

## Mod sync

- **mode**: `delta` (preserve user-added/removed mods), `mirror` (exact match), `mirror_fast` (mirror with size-only checks).
- **required**: Mod IDs that must stay installed; removed locally they are restored.
- **blocked**: Mod IDs that must not appear in the pack.
- **optional_sets**: Toggleable optional mod groups users can enable/disable in the launcher.

Mods are managed separately from `content_rules`; do not add a `mods` directory content rule.

## Resource sync

Controls verification of client jar, libraries, and assets (separate from mod sync):

- `on_update` — check on instance update only (default)
- `always` — check on update and launch
- `always_fast` — like `always`, but size-only checks where possible

## Authentication providers

- `"mojang"`: Official Mojang auth. No extra fields.
- `"telegram"`: [tgauth](https://foxlab.dev/minecraft/tgauth-backend). Requires `"auth_base_url"`.
- `"ely.by"`: [ely.by](https://ely.by). Requires `"client_id"`, `"client_secret"`, and optionally `"launcher_name"`.
- `"offline"`: Offline mode.
