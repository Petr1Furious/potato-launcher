# Hosting

The launcher only needs static files served over HTTPS. Nothing runs on the host besides a web server; instances are built on the machine that publishes them.

## Site layout

One folder is served as the site root:

```
index.html                  # downloads page, written by `potato publish-launcher`
launcher/<os>/<file>        # launcher builds
launcher/<os>/version.txt
data/instance_manifest.json # the instance list launchers read
data/instances/<id>/meta.json
data/instances/<id>/publish.json
data/history/<id>/...
data/objects/<ab>/<sha1>    # every stored file, named by its sha1
```

Instance repos publish into `data/` (their `[publish] target`), and launcher builds are published to the site root.

## Docker Compose

::: info Requirements
- A Linux server with a public IP address
- A domain name
- Docker + Docker Compose plugin installed
:::

1. Download `docker-compose.yml`, `.env.example` and the nginx config from the repository:

```bash
mkdir potato-site && cd potato-site
wget https://raw.githubusercontent.com/Petr1Furious/potato-launcher/refs/heads/master/docker-compose.yml
wget -O .env https://raw.githubusercontent.com/Petr1Furious/potato-launcher/refs/heads/master/.env.example
mkdir -p docker/nginx/templates
wget -O docker/nginx/templates/default.conf.template https://raw.githubusercontent.com/Petr1Furious/potato-launcher/refs/heads/master/docker/nginx/templates/default.conf.template
wget -O docker/nginx/10-launcher-lower-name.envsh https://raw.githubusercontent.com/Petr1Furious/potato-launcher/refs/heads/master/docker/nginx/10-launcher-lower-name.envsh
```

2. Edit `.env`:

- **`SITE_DIR`**: The folder to serve. Default: `./site`
- **`LAUNCHER_NAME`**: Must match the name the launcher was built with. It's used to map old launcher update URLs (see below).
- **`PORT`**: Host port nginx listens on. Default: `8000`
- **`CONTAINER_NAME_PREFIX`**: Prefix for Docker container names. Default: `potato-launcher`

3. Start it:

```bash
docker compose up -d
```

4. Put a reverse proxy with TLS in front of it (Caddy, Traefik, or nginx with Certbot) and point it at `http://127.0.0.1:<PORT>`.

The nginx config serves `data/objects/` with immutable cache headers and the manifests without caching. It also maps the URLs that legacy launchers use to update themselves (`/api/v1/launchers/...` and `/version_<os>.txt`) onto `launcher/`.

Any other static web server works too, but without the URL mapping, older launchers stop updating themselves.

## Publish targets

`potato` writes to the site folder through one of these targets:

| Target | Example | Notes |
|---|---|---|
| Local folder | `file:///srv/potato/site/data` | For publishing on the host itself |
| SSH | `rsync://user@host[:port]/srv/potato/site/data` | Needs `rsync` and `ssh` locally, and `flock` and GNU `find` on the host |
| S3 | `s3://bucket/prefix/data` | S3, R2 or any S3-compatible storage |

Credentials come from the usual places: your SSH config and agent for `rsync://`, and `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION` and `AWS_ENDPOINT` (or `~/.aws`) for `s3://`. With S3, serve the bucket through any HTTPS front, like a public bucket URL or a CDN.

Every command that writes to storage takes a lock, so two people publishing at once can't break the instance list. Ctrl-C releases the lock.

::: warning
If potato crashes while publishing to S3, the lock may become stuck. Delete `.potato-lock` from the bucket to release it.
:::

## Backups

You can rebuild the whole site by publishing each instance repo again, since jars only you host are committed in the repos. The exception is repos with `self_hosted = "storage"`: their CurseForge and custom files only exist in `data/objects/` and in the Prism instance they came from. Back up `data/objects/` if you use it.
