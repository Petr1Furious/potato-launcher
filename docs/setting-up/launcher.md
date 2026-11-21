# Launcher configuration

Currently, the only supported deployment method is GitHub Actions, as it's the easiest way to build for all OSes.

::: tip You don't have to build your own launcher
Players can use the main Potato Launcher and add your instance list to it: click **Configure backends**, paste `https://<your-domain>/data/instance_manifest.json` under **Add Backend** and click **Add Backend**. The main page of your site shows this URL with a Copy button. This works as long as there were no breaking changes between your `potato` version and the main launcher's.
:::

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

::: warning Change the app ID and name
After forking, set your own **LAUNCHER_APP_ID** and **LAUNCHER_NAME**. macOS and Flatpak use the app ID to tell apps apart, so with the original ID your launcher and Potato Launcher will conflict on players' systems. The name sets the launcher's data folder by default, so with the original name both launchers share their instances and settings.
:::

- **LAUNCHER_NAME** (required): Your launcher name, for example "Potato Launcher". Can only contain Latin letters, numbers, spaces, `-`, `_` and `'`. Change it from the original, see the warning above
- **INSTANCE_MANIFEST_URLS** (optional): One or more instance manifest URLs for your deployed modpacks, separated by commas, semicolons, or newlines. For a typical server deployment, set this to `https://<your-domain>/data/instance_manifest.json`. The launcher uses these as default catalog URLs; users can add or remove catalogs later in launcher settings. Can be omitted if you want users to configure catalogs manually after install.
- **LAUNCHER_APP_ID** (required): An application ID in a [reverse domain notation](https://en.wikipedia.org/wiki/Reverse_domain_name_notation). Used in macOS and Flatpak packages. For example, `com.example.MyLauncher`. Use your own, see the warning above
- **LAUNCHER_ICON** (required): A path to the launcher icon, relative to the repository root. For example, `packaging/potato_launcher.png`

  If you are using the environment variables option, this can also be a URL.
- **LAUNCHER_DATA_DIR** (optional): The name of the folder in the user's data directory where the launcher keeps its data. Each launcher gets its own folder. Default: the launcher name in lowercase with dashes, for example `potato-launcher`

  Users can also override the full path with the `POTATO_LAUNCHER_DATA_DIR` environment variable when running the launcher.
- **LAUNCHER_UPDATE_URL** (optional): Where the launcher looks for its own updates on your host. Set it to `https://<your-domain>/launcher`. The launcher reads `<url>/<os>/version.txt` and downloads `<url>/<os>/<file>`.
- **LAUNCHER_UPDATE_GITHUB** (optional): Update from GitHub Releases instead, as `owner/repo`. The launcher reads the latest release and uses its tag as the version.

  Set at most one of the two, setting both fails the build. Without either, the launcher doesn't update itself. Variables from Actions only override `build.env` when they're non-empty, so if you set `LAUNCHER_UPDATE_GITHUB` there, also remove `LAUNCHER_UPDATE_URL` from `build.env`.
- **LAUNCHER_DESCRIPTION** (optional): The application description. Used in `.desktop` files in the Nix and Flatpak packages, can safely be omitted.
- **FLATPAK_KEYWORDS** (optional): Semicolon-separated list of additional keywords for the Flatpak `.desktop` file. Can safely be omitted.
- **USE_NATIVE_GLFW_DEFAULT** (optional): If `true`, new instances default to using native GLFW. Default: `false`

## Publishing releases

The launcher is built and released when you push a version tag:

```bash
git tag v1.2.0
git push --tags
```

The tag becomes the launcher's version. Use `vX.Y.Z` tags (pre-releases like `v1.2.0-rc1` work too): launchers only offer an update when the published version is newer than their own. Other versions update on any difference.

The **Release** job then creates a GitHub release with the launcher files and the `potato` binaries. If `LAUNCHER_PUBLISH_TARGET` is set, it also publishes the launcher to your host with `potato publish-launcher`, which updates the downloads page at the site root.

To publish to your host, set these variables:

- **LAUNCHER_PUBLISH_TARGET**: The site root (not `data/`), for example `rsync://user@host/srv/potato/site` or `s3://bucket/prefix`. See [publish targets](/setting-up/hosting#publish-targets).
- **LAUNCHER_PUBLISH_KNOWN_HOSTS**: The host's SSH key lines for `known_hosts`, for `rsync://` targets.
- **AWS_REGION**, **AWS_ENDPOINT**: For `s3://` targets.

And these secrets:

- **LAUNCHER_PUBLISH_SSH_KEY**: A private SSH key with write access to the site folder, for `rsync://` targets.
- **AWS_ACCESS_KEY_ID**, **AWS_SECRET_ACCESS_KEY**: For `s3://` targets.
- **GPG_PRIVATE_KEY**, **GPG_PASSPHRASE** (optional): Used to sign the Flatpak bundle.

To add them, go to the "Settings" tab, then choose "Secrets and variables" on the left and select "Actions".

## Triggering the build

Make sure to enable actions in your fork by going to the "Actions" tab and clicking "I understand my workflows".

Then, the build runs:

- On every push to `master`, keeping the builds as workflow artifacts
- On every `v*` tag, which also makes a release
- Manually via "Run workflow" on **Build launcher**, for any branch. This only keeps the builds as workflow artifacts, without a release.

After a release, the launcher is available at `https://<your-domain>/` and `https://<your-domain>/launcher/<os>/`.
