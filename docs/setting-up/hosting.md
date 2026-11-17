# Hosting

The launcher only needs static files over HTTPS. Nothing runs on the host
besides a web server: instances are built on the machine that publishes them.

## Site layout

One folder is served as the site root:

```
index.html                    # downloads page, written by `potato publish-launcher`
launcher/<os>/<file>          # launcher builds
launcher/<os>/version.txt
data/instance_manifest.json   # the instance list launchers read
data/instances/<id>/meta.json
data/instances/<id>/publish.json
data/history/<id>/...
data/objects/<ab>/<sha1>      # every stored file, named by its sha1
```

Instance repos publish into `data/` (their `[publish] target`). Launcher builds
are published to the site root.

## Docker Compose

The repository has a `docker-compose.yml` with one nginx container.

```bash
mkdir potato-site && cd potato-site
wget https://raw.githubusercontent.com/Petr1Furious/potato-launcher/refs/heads/master/docker-compose.yml
wget -O .env https://raw.githubusercontent.com/Petr1Furious/potato-launcher/refs/heads/master/.env.example
mkdir -p docker/nginx/templates
wget -O docker/nginx/templates/default.conf.template https://raw.githubusercontent.com/Petr1Furious/potato-launcher/refs/heads/master/docker/nginx/templates/default.conf.template
wget -O docker/nginx/10-launcher-lower-name.envsh https://raw.githubusercontent.com/Petr1Furious/potato-launcher/refs/heads/master/docker/nginx/10-launcher-lower-name.envsh
docker compose up -d
```

`.env`:

- **`SITE_DIR`**: the folder to serve. Default: `./site`
- **`LAUNCHER_NAME`**: must match the name the launcher was built with. Used
  to map old launcher update URLs (see below).
- **`PORT`**: host port. Default: `8000`
- **`CONTAINER_NAME_PREFIX`**: default `potato-launcher`

Put a reverse proxy with TLS in front of it (Caddy, Traefik, or nginx with
Certbot) and point it at `http://127.0.0.1:<PORT>`.

The nginx config:

- serves `data/objects/` with immutable cache headers, and manifests without caching
- maps the URLs that launchers built before the backend removal use for
  self-update (`/api/v1/launchers/...`, `/version_<os>.txt`) onto `launcher/`

Any other static web server works too. Without the URL mapping, older
launchers just stop updating themselves.

## Publish targets

`potato` writes to the site folder through one of these targets:

| Target | Example | Notes |
|---|---|---|
| Local folder | `file:///srv/potato/site/data` | publishing on the host itself |
| SSH | `rsync://user@host[:port]/srv/potato/site/data` | needs `rsync` and `ssh` locally, `flock` and GNU `find` on the host |
| S3 | `s3://bucket/prefix/data` | S3, R2 or any S3-compatible store |

Credentials come from the usual places: your SSH config and agent for
`rsync://`, and `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION` and
`AWS_ENDPOINT` (or `~/.aws`) for `s3://`. With S3, serve the bucket through
any HTTPS front (a public bucket URL or a CDN).

Every command that writes to storage takes a lock, so two people publishing at
once can't break the instance list. Ctrl-C releases it. If potato crashes while
publishing to S3, delete `.potato-lock` from the bucket.

## Backups

Most of the site can be rebuilt by publishing each instance repo again. The
exception is jars uploaded by `potato pull prism` or `potato mod add` (CurseForge
and custom jars): they only exist in `data/objects/` and in the Prism instance
they came from. Back up `data/objects/` or keep those jars.
