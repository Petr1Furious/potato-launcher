# Launcher configuration

Currently, the only supported deployment method is GitHub Actions, as it's the easiest way to build for all OSes.

Start by forking the https://github.com/Petr1Furious/potato_launcher repository.

Then, you have two options to configure your launcher: config file or environment variables.

### Config file

::: info Pros and cons
- Pro: Everything needed to build the launcher is inside the repo, so you
  (or your users) can clone the repo and build the launcher right away
- Con: May be harder to update when the upstream updates
:::

To use this option, edit the `build.env` file at the root of the repository. Each line corresponds to a variable. Everything before `=` is the name, everything after is a value. Comments are not supported, but empty lines are ignored.

This file is read automatically when building the launcher (compile time), by packaging scripts, by the Nix package, and by GitHub Actions when a repository Variable is unset. Variables set in GitHub Actions take priority over `build.env`.

### Environment variables

::: info Pros and cons
- Pro: Doesn't require changing files in the repository, so it's easier to update your repo when the upstream updates
- Con: Nix package won't work out of the box
:::

To use this option, go to the "Settings" tab, then choose "Secrets and variables" on the left and select "Actions". Then, add your config variables in the "Variables" tab. Make sure to not accidentally create them as secrets instead.


## Variables list

- **LAUNCHER_NAME** (required): Your launcher name, for example "Potato Launcher". Can only contain Latin letters, numbers, spaces, `-`, `_` and `'`
- **INSTANCE_MANIFEST_URLS** (optional): One or more instance manifest URLs for your deployed modpacks, separated by commas, semicolons, or newlines. For a typical server deployment, set this to `https://<your-domain>/data/instance_manifest.json`. The launcher uses these as default backend catalog URLs; users can add or remove catalogs later in launcher settings. Can be omitted if you want users to configure catalogs manually after install.
- **LAUNCHER_APP_ID** (required): An application ID in a [reverse domain notation](https://en.wikipedia.org/wiki/Reverse_domain_name_notation). Used in macOS and Flatpak packages. For example, `me.petr1furious.PotatoLauncher`
- **LAUNCHER_ICON** (required): A path to the launcher icon, relative to the repository root. For example, `packaging/potato_launcher.png`

  If you are using the environment variables option, this can also be a URL.
- **BACKEND_API_BASE** (optional): An URL that will be used to download launcher updates. Doesn't impact instance download. Set it to `https://<your-domain>/api/v1` if you want the launcher to update automatically (you want to). Also used to generate the `.flatpakref` file
- **LAUNCHER_DESCRIPTION** (optional): The application description. Used in `.desktop` files in the Nix and Flatpak packages, can safely be omitted.
- **FLATPAK_KEYWORDS** (optional): Semicolon-separated list of additional keywords for the Flatpak `.desktop` file. Can safely be omitted.
- **USE_NATIVE_GLFW_DEFAULT** (optional): If `true`, new instances default to using native GLFW. Default: `false`

## Setting up secrets

If you want GitHub Actions to upload built launcher artifacts to your backend automatically, set these secrets.

Go to the "Settings" tab, then choose "Secrets and variables" on the left and select "Actions". Then, add the secrets in the "Secrets" tab.

You need to set the following secrets:
- **ADMIN_SECRET_TOKEN**: Must match the `ADMIN_SECRET_TOKEN` used by your deployed backend (see [Server setup](/setting-up/server))

If you want signed Flatpak packages on `master`, also set:
- **GPG_PRIVATE_KEY**: Private key used to sign Flatpak bundles
- **GPG_PASSPHRASE**: Passphrase for the key above, if any

And the corresponding variable:
- **GPG_PUBLIC_KEY**: Public key matching the private key above

## Triggering the build

All done! Make sure to enable actions in your fork by going to the "Actions" tab and clicking "I understand my workflows".

Then, you can trigger the build:

- By pushing to the `master` branch. This runs the **Check** workflow first, and when it succeeds, the **Build launcher** workflow starts automatically. Uploads happen on `master` when `BACKEND_API_BASE` and `ADMIN_SECRET_TOKEN` are set.
- Or manually via "Run workflow" on **Build launcher** (you can enable uploads in the workflow inputs)

After a successful workflow run, the launcher binaries and packages should be available at `https://your.domain/launcher`
