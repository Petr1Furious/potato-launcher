# Server setup (Docker Compose)

The backend + filebrowser + nginx (with frontend) can be deployed with one `docker-compose.yml`.

::: info Requirements
- A Linux server with a public IP address
- A domain name
- Docker + Docker Compose plugin installed
:::

## Quickstart

1. Download `docker-compose.yml` and `.env.example` from the repository.

```bash
mkdir potato-launcher-backend
cd potato-launcher-backend
wget https://raw.githubusercontent.com/Petr1Furious/potato-launcher/refs/heads/master/docker-compose.yml
wget -O .env https://raw.githubusercontent.com/Petr1Furious/potato-launcher/refs/heads/master/.env.example
```

2. Edit `.env` and set at least:

- **`ADMIN_JWT_SECRET`**: JWT signing secret (generate with `openssl rand -base64 48`)
- **`ADMIN_SECRET_TOKEN`**: admin “password” used to log in (pick a long random password)
- **`DOWNLOAD_SERVER_BASE`**: public URL where your server will host generated instance files (usually `https://<your-domain>/data`)

Other useful variables from `.env.example`:

- **`REPLACE_DOWNLOAD_URLS`**: if `true`, instance builds serve libraries and assets from your server instead of Mojang/upstream URLs. Can also be toggled later in the admin **Settings** page. Default: `false`
- **`RESOURCES_URL_BASE`**: optional override for the assets base URL passed to the builder
- **`VITE_GITHUB_URL`**: repository link shown on the public downloads page
- **`PORT`**: host port nginx listens on. Default: `8000`
- **`PUID`** / **`PGID`**: user/group IDs for container file ownership. Default: `1000`
- **`CONTAINER_NAME_PREFIX`**: prefix for Docker container names. Default: `potato-launcher`

3. Start the setup:

```bash
docker compose up -d
```

4. Setup a reverse proxy.

- By default, nginx is exposed on **port 8000** on the host (see `docker-compose.yml`). You can change `PORT` to any other value.
- Choose any reverse proxy and setup TLS certificates. A couple of common options: [Caddy](https://caddyserver.com/docs/), [Traefik](https://doc.traefik.io/traefik/) (both with automatic Let’s Encrypt) and [nginx](https://nginx.org/en/docs/beginners_guide.html) + [Certbot](https://certbot.eff.org/instructions) (more manual approach).
- Point the proxy at `http://127.0.0.1:8000` (or whatever `PORT` you chose). The URLs below assume you access the site through the proxy on standard HTTPS, without the port in the URL.

## What gets served where

When the setup is up:

- **Downloads page**: `https://<your-domain>/`
- **Backend API**: `https://<your-domain>/api/v1/`
- **API docs**: `https://<your-domain>/api/v1/docs`
- **Generated instances & metadata**: `https://<your-domain>/data/`
- **Admin panel**: `https://<your-domain>/admin/` (requires login)
- **Filebrowser** (uploaded instance files): `https://<your-domain>/filebrowser/` (requires login)


## Logging in

Open `https://<your-domain>/admin` and log in using your `ADMIN_SECRET_TOKEN`.

## Persistent data / backups

All persistent state lives under `./state/` by default:

- `state/internal`: instances spec + uploaded instances storage
- `state/generated`: generated output served under `/data`
- `state/workdir`: builder working directory (can be thought of as a cache for the builder)
- `state/launcher`: uploaded launcher artifacts
- `state/filebrowser-db`: Filebrowser database

Back up the whole `state/` directory.

::: warning
Do not share `ADMIN_SECRET_TOKEN` or `ADMIN_JWT_SECRET`. Anyone with the admin token can log in and upload/modify instances and launchers.
:::
