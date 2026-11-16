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
- **INSTANCE_MANIFEST_URLS** (optional): One or more instance manifest URLs for your deployed modpacks, separated by commas, semicolons, or newlines. For a typical server deployment, set this to `https://<your-domain>/data/instance_manifest.json`. The launcher uses these as default catalog URLs; users can add or remove catalogs later in launcher settings. Can be omitted if you want users to configure catalogs manually after install.
- **LAUNCHER_APP_ID** (required): An application ID in a [reverse domain notation](https://en.wikipedia.org/wiki/Reverse_domain_name_notation). Used in macOS and Flatpak packages. For example, `me.petr1furious.PotatoLauncher`
- **LAUNCHER_ICON** (required): A path to the launcher icon, relative to the repository root. For example, `packaging/potato_launcher.png`

  If you are using the environment variables option, this can also be a URL.
- **LAUNCHER_UPDATE_URL** (optional): Where the launcher looks for its own updates, on your host. Set it to `https://<your-domain>/launcher`. The launcher reads `<url>/<os>/version.txt` and downloads `<url>/<os>/<file>`.
- **LAUNCHER_UPDATE_GITHUB** (optional): Update from GitHub Releases instead, as `owner/repo`. The launcher reads the latest release and uses its tag as the version.

  Set at most one of the two. Setting both fails the build. Without either, the launcher doesn't update itself. Variables from Actions only override `build.env` when non-empty, so if you set `LAUNCHER_UPDATE_GITHUB` there, also remove `LAUNCHER_UPDATE_URL` from `build.env`.
- **LAUNCHER_DESCRIPTION** (optional): The application description. Used in `.desktop` files in the Nix and Flatpak packages, can safely be omitted.
- **FLATPAK_KEYWORDS** (optional): Semicolon-separated list of additional keywords for the Flatpak `.desktop` file. Can safely be omitted.
- **USE_NATIVE_GLFW_DEFAULT** (optional): If `true`, new instances default to using native GLFW. Default: `false`

## Publishing releases

Every push builds the launcher, and the builds are kept as workflow artifacts.
A release happens only when you push a version tag:

```bash
git tag v1.2.0
git push --tags
```

The tag becomes the launcher's version. The **Release** job then creates a
GitHub release with the launcher files and `potato` binaries, and, if
`LAUNCHER_PUBLISH_TARGET` is set, publishes the launcher to your host with
`potato publish-launcher`. That also regenerates the downloads page at the
site root.

Variables for publishing to your host:

- **LAUNCHER_PUBLISH_TARGET**: the site root (not `data/`), e.g.
  `rsync://user@host/srv/potato/site` or `s3://bucket/prefix`. See
  [publish targets](/setting-up/hosting#publish-targets).
- **LAUNCHER_PUBLISH_KNOWN_HOSTS**: the host's SSH key line(s) for
  `known_hosts`, for `rsync://` targets.
- **AWS_REGION**, **AWS_ENDPOINT**: for `s3://` targets.

Secrets:

- **LAUNCHER_PUBLISH_SSH_KEY**: private SSH key with write access to the site
  folder, for `rsync://` targets.
- **AWS_ACCESS_KEY_ID**, **AWS_SECRET_ACCESS_KEY**: for `s3://` targets.
- **GPG_PRIVATE_KEY**, **GPG_PASSPHRASE** (optional): sign the Flatpak bundle.

Go to "Settings" → "Secrets and variables" → "Actions" to add them.

## Triggering the build

Enable actions in your fork in the "Actions" tab. Builds then run after the
**Check** workflow succeeds on a push, on every `v*` tag, or manually via "Run
workflow" on **Build launcher**.

After a release, the launcher is available at `https://<your-domain>/` and
`https://<your-domain>/launcher/<os>/`.
