# Creating instances

**Note on terminology:** Potato Launcher calls these **instances**. In most cases instances will be modpacks, but technically you can deploy vanilla versions too.

## Recommended: use the new Web UI

If you deployed the full server setup (see [Server setup](/setting-up/server)), you can manage instances from the browser. Typical workflow:

- Login into the admin panel at `https://<your-domain>/admin`
- Click **New Instance** or select an instance you want to update and click **Update**
- Fill all necessary fields (**Instance Name**, **Minecraft Version**, **Mod Loader**, **Loader version** (if not vanilla) and **Authentication Type**)
- Click **Create Instance**
- To upload and include files (e.g. mods) select an existing instance, click **Update** and **Manage instance files**. Upload all files you want to include (e.g. `mods` directory with mods in it)
- **Important:** add include rules and specify **Path** and rule type. This step is necessary because the launcher doesn't know what to do with each uploaded file/directory when it has been changed on the backend. For example, you may want to set **Recursive** on `config`, and both **Overwrite** and **Delete Extra** on `mods`. The detailed description of different rule types is available in the [Instance fields](#instance-fields) section
- Click **Build** to generate the `/data/` output served by nginx

The launcher will download metadata from the version manifest at `<DOWNLOAD_SERVER_BASE>/version_manifest.json` (which is usually `https://<your-domain/data/version_manifest.json`)

## Manual

Linux/macOS is recommended for building instances. However, Windows should also work

If you don't have Rust installed, get it from [rustup.rs](https://rustup.rs)

Clone the repository and go to the `instance_builder` directory:
```bash
git clone <your-repository-url>
cd <repository-name>/instance_builder
```

Then, you'll need to create a `spec.json` file. It's used to define launcher instances that should be created. The file format is described below. You can also find an example config at [`instance_builder/spec.example.json`](https://github.com/Petr1Furious/potato_launcher/blob/master/instance_builder/spec.example.json)

After defining your instance, you can build it with the following command:

```bash
cargo run --release -p instance_builder -- -s <path to spec.json>
```

This will create a `generated` directory, which should then be uploaded to your server. If you followed the [Server configuration](/setting-up/server) guide, you should upload the contents of this directory (not the directory itself) to the `data` subdirectory of your launcher dir, e.g. to `/srv/potatosmp/data`. You can use the `exec_after_all` setting to automate this process.

## Manual (remote server build via SSH)

If you already have the backend deployed and you want to automate uploading files and building instances, you can:

- Pick the backend **internal directory** on the server, e.g. `/srv/potato-launcher/state/internal`
- Upload your `spec.json` into `<internal-dir>/spec.json`
- Upload your raw modpack files into `<internal-dir>/uploaded-instances/<instance-name>/`
- Run `instance_builder` inside the running backend container (`potato-launcher-backend` by default)

This repository includes a helper script that automates the above, see `./scripts/remote-instance-build.sh --help` for all options.

::: warning
The helper script **does not** modify `include_from`. If your instances use `include`, you must set `include_from` in `spec.json` to the correct **in-container** path that points at the uploaded files (default is `/data/internal/uploaded-instances/<instance-name>`).
:::

## JSON structure

```json
{
  "download_server_base": "string",
  "resources_url_base": "string",
  "replace_download_urls": "boolean",
  "version_manifest_url": "string",
  "instances": [
    {
      "name": "string",
      "minecraft_version": "string",
      "loader_name": "string",
      "loader_version": "string",
      "include_from": "string",
      "include": [
        {
          "path": "string",
          "overwrite": "boolean",
          "recursive": "boolean",
          "delete_extra": "boolean"
        },
        <...>
      ],
      "auth_backend": {
        "type": "string",
        "data_field1": "data_value1",
        "data_field2": "data_value2",
        <...>
      },
      "recommended_xmx": "string",
      "exec_before": "string",
      "exec_after": "string"
    }
  ],
  "exec_before_all": "string",
  "exec_after_all": "string"
}
```

## Fields


### Root Fields

- **download_server_base** (required): The base URL where the instance will be deployed. All files in the generated folder (`generated` by default) must be accessible by `<download_server_base>/<file_relative_path>` after deployment. For example, the version manifest has to be at `<download_server_base>/version_manifest.json`. You probably want this set to `https://your.domain/data`
- **resources_url_base**: The base URL for assets. Should be equal to `<download_server_base>/assets/objects` if the generated folder structure is not changed after upload. If omitted, the launcher will download assets from Mojang servers. Unset by default
- **replace_download_urls**:
  If set to `true`, all instance files will be downloaded from your server.
  
  If set to `false`, the original download URLs will be kept when possible. This means that assets, libraries, modloaders and the Minecraft jar will be downloaded from their original locations and only metadata, files specified in `include`, and (Neo)Forge patched jars will be downloaded from your server.
  
  Default: `false`
- **version_manifest_url**: The URL from which to fetch a remote version manifest. If specified, the instance builder will fetch the existing manifest from this URL and merge the local versions with it, preserving any versions that exist in the remote manifest but not in the local specification.

  In other words, set this to `<download_server_base>/version_manifest.json` if you want to manage different instances from different devices (for example, when you have multiple server admins responsible for different servers).
- **instances** (required): An array of instance specification objects (see below for details).
- **exec_before_all**: A console command to execute before processing all versions.
- **exec_after_all**: A console command to execute after processing all versions. This is useful for automatically deploying the generated files (for example, by `rsync`'ing them to a server with `nginx`).

  If you are running Nginx and have it serving `/data` from `/srv/potato_launcher/generated` on your server, you can use something like:
  `chmod -R +r ./generated && rsync -vza ./generated/ user@server:/srv/potato_launcher/generated/`

  Note the trailing slashes; they matter in rsync!

### Instance Fields

- **name** (required): The name of the instance.
- **minecraft_version** (required): The Minecraft version for this instance.
- **loader_name**: The name of the modloader ("vanilla", "fabric", "forge" or "neoforge"). Default: `"vanilla"`
- **loader_version**: The version of the modloader. Defaults to `"latest"` for Fabric and `"recommended"` for (Neo)Forge
- **include**: An array of inclusion rules. Each rule is an object with the following fields:
  - **path** (required): The file or directory (relative to the `include_from` directory) to include.
  - **overwrite**: A boolean indicating if the included file(s) should always be overwritten. Default: `true`
  - **delete_extra**: If set to true along with `overwrite: true`, extra files in the target directory will be deleted. Default: `true`
  - **recursive**:
    If set to `true`, missing files from this directory will be re-downloaded every time the instance is synchronized. If set to `false`, this directory will be ignored after it's downloaded for the first time. Has no effect on files or with `overwrite: true`. Default: `false`.
- **include_from**: A directory from which to include files. For example, it can be a path to a PrismLauncher instance with your modpack. Required if `include` contains entries.
- **auth_backend**: The Minecraft authentication provider required for this instance. If omitted, any provider can be selected by users. See below for the list of providers and their config settings
  - **type**: The authentication provider name
  - Any additional fields for the selected authentication provider
- **recommended_xmx**: The instance's default JVM RAM limit (`-Xmx`). Should be a string with `M` or `G` suffix (for example, "8192M"). If no suffix is given, `M` is assumed. Currently defaults to `4096M` when unset
- **exec_before**: A command to execute before processing this instance
- **exec_after**: A command to execute after processing this instance

## Authentication providers

Currently, the following authentication backends/providers are supported:

- `"mojang"`: The official authentication server. Requires no parameters
- `"telegram"`: [tgauth](https://foxlab.dev/minecraft/tgauth-backend). Requires `"auth_base_url"` parameter to be set to the base URL of the tgauth server, e.g. `"https://your.auth.server"`
- `"ely.by"`: [ely.by](https://ely.by). To use this provider, you need to create a "Web site" application at https://account.ely.by/dev/applications. Parameters: `"client_id"`, `"client_secret"`, `"launcher_name"` (human-readable string shown after successful login)
