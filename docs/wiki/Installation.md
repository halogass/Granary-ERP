# Installation

<!-- nav:top -->
[Home](Home) › Getting started › **Installation**

**On this page:** [Requirements](#requirements) · [What runs where](#what-runs-where) · [Install with Docker Compose](#install-with-docker-compose) · [Reverse proxy and TLS (Nginx)](#reverse-proxy-and-tls-nginx) · [TURN server for video calls (coturn)](#turn-server-for-video-calls-coturn) · [Building the images on another machine](#building-the-images-on-another-machine) · [WhatsApp (optional)](#whatsapp-optional)
<!-- /nav:top -->

This page takes you from an empty Linux server to a running Granary that you can sign in to. It is for the
person who runs the server. Granary runs as a set of Docker containers started by Docker Compose, with Nginx on
the host in front of them. When you finish here, continue with [First Steps](First-Steps).

---

## Requirements

### Server

| Resource | Minimum | Recommended |
|---|---|---|
| CPU | 2 cores | 4 cores |
| RAM | 4 GB | 8 GB (the PDF service runs headless Chromium; face matching and speech to text use a lot of CPU) |
| Disk | 20 GB | 50 GB SSD |
| OS | A current Linux with Docker | Ubuntu 22.04 LTS or later, or Debian 12 |

Compiling the Rust images needs about **8 GB of RAM and 10 GB of disk** for the build cache. On a smaller server,
build the images on another machine and copy them over (see
[Building the images on another machine](#building-the-images-on-another-machine)).

### Software

- Docker Engine 24 or later with the Compose plugin (v2). Check with `docker --version` and
  `docker compose version`.
- Nginx on the host (installed with your package manager), as the reverse proxy.
- Git, to fetch the source.
- Optional: Certbot (Let's Encrypt) or `cloudflared` (Cloudflare Tunnel) for a public domain.

---

## What runs where

All containers publish their ports on `127.0.0.1` only. Nginx on the host listens on ports 80 and 443 and forwards
requests to them.

| Service | What it does | Host port |
|---|---|---|
| `frontend` | The Next.js web application | 127.0.0.1:3000 |
| `backend` | `granary-api`, the HTTP API. On every start it first applies migrations and the seed | 127.0.0.1:8000 |
| `worker` | Runs background jobs (mail polling, reminders, FX sync, Nary) | none |
| `beat` | Puts scheduled jobs on the queue (same image as `backend`) | none |
| `db` | PostgreSQL 16 with pgvector (embeddings for AI search) | none |
| `redis` | Redis 7: cache, job queue, live updates, locks | none |
| `storage` | SeaweedFS, S3-compatible file storage (uploads, documents, recordings) | 127.0.0.1:9000 |
| `pdf` | Renders documents to PDF with headless Chromium | none |
| `face` | Face matching and liveness checks for clock-in | none |
| `research` | Reads web pages for CRM enrichment and research, behind a filtering proxy | none |
| `searxng` | Self-hosted web search | none |
| `stt` | Speech to text for voice notes (faster-whisper on the CPU) | none |
| `signal` | WebRTC signalling for video calls | 127.0.0.1:3003 |
| `coturn` | TURN/STUN server so video calls work across networks | host network |

The `research` and `searxng` services sit on their own Docker network. They can't reach the database, Redis or
the storage.

---

## Install with Docker Compose

### 1. Get the source

```bash
git clone <your-repo-url> granary
cd granary
cp .env.example .env
```

### 2. Fill in `.env`

Open `.env` in an editor and replace every value marked `← CHANGE THIS`. Generate **each secret separately**:

```bash
openssl rand -hex 32
```

The values you must set before the first start:

```dotenv
# Keys (each its own random value)
APP_SECRET_KEY=<random>                 # the master key: set it once, before real data (see below)
JWT_SECRET_KEY=<random, 32+ characters>
NEXTAUTH_SECRET=<random>
SIGNAL_TICKET_KEY=<random>
INTERNAL_AUTH_TOKEN=<random>

# Database
POSTGRES_PASSWORD=<strong password>
DATABASE_URL=postgresql://granary:<same password>@db:5432/granary

# Redis: the URLs must carry the password
REDIS_PASSWORD=<random>
REDIS_URL=redis://:<redis password>@redis:6379/0
CELERY_BROKER_URL=redis://:<redis password>@redis:6379/1
CELERY_RESULT_BACKEND=redis://:<redis password>@redis:6379/2

# S3 storage settings (the names date from the earlier MinIO setup)
MINIO_SECRET_KEY=<strong password>

# Tokens used only between containers
PDF_SERVICE_TOKEN=<random>
FACE_SERVICE_TOKEN=<random>
RESEARCH_SERVICE_TOKEN=<random>
STT_SERVICE_TOKEN=<random>
SEARXNG_SECRET=<random>

# The first account (the Owner), created on the first start
SUPERADMIN_EMAIL=owner@example.com
SUPERADMIN_NAME=Hana Mori
SUPERADMIN_PASSWORD=<strong initial password>

# Your public URLs
APP_URL=https://granary.example.com
SIGNAL_URL=wss://granary.example.com/ws/signal
MINIO_PUBLIC_URL=https://granary.example.com/minio
CORS_ORIGINS=["https://granary.example.com"]

# Keep these as they are (Docker-internal addresses)
INTERNAL_API_URL=http://backend:8000
API_URL=http://backend:8000

# TURN server for video calls
TURN_SECRET=<random>
TURN_EXTERNAL_IP=<the server's public IP>
TURN_URIS=["turn:granary.example.com:3478","turns:granary.example.com:5349"]

# Text inside the e-signature seal
SIGN_SEAL_NAME="Kumo Trading Co."
SIGN_LOCATION=Tokyo
```

Things to know about `.env`:

- **Placeholder secrets stop the start.** The API and the worker refuse to start while `JWT_SECRET_KEY`,
  `APP_SECRET_KEY` or `NEXTAUTH_SECRET` still begins with the `.env.example` placeholder (`change-this`). The log
  names the variable, never its value. Other weak secrets (short keys, values starting with `test`, `password`,
  `example` and similar) don't stop the start: they are logged and the Owner is alerted once per start.
- **Compose itself refuses to start** without `REDIS_PASSWORD`, `NEXTAUTH_SECRET` and `SIGNAL_TICKET_KEY`.
- **`APP_SECRET_KEY` encrypts secrets stored in Settings** (mail passwords, AI keys). Set it once, before you
  enter real data: settings encrypted with one key can't be read with another.
- **`INTERNAL_API_URL` and `API_URL` must stay `http://backend:8000`.** The public domain there causes an
  http-to-https redirect loop that breaks API calls.
- **`API_URL` and `SIGNAL_URL` are baked into the frontend when it is built.** After changing them, rebuild the
  frontend (`docker compose up -d --build frontend`).
- Business settings (taxes, payroll, approval limits, time zone) are **not** environment variables. You set them
  in the web interface after the first sign-in.

All variables are described in [Configuration](Configuration#environment-variables).

#### Trying Granary on your own computer

To test locally without Nginx, use these values instead of the public URLs:

```dotenv
APP_URL=http://localhost:3000
SIGNAL_URL=ws://localhost:3003
CORS_ORIGINS=["http://localhost:3000"]
MINIO_PUBLIC_URL=http://localhost:9000
```

Then open `http://localhost:3000` after the first start.

### 3. Build and start

```bash
docker compose up -d --build
```

The Rust services (`backend`, `worker`, `beat`, `pdf`, `face`, `signal`, `research`) run images built from
`rust-version/Dockerfile` under fixed names (`granary-rust-api`, `granary-rust-pdf`, `granary-rust-face`,
`granary-rust-signal`, `granary-rust-research`, tag `latest`). Compose never tries to download them. The
`frontend` and `stt` images are built from `frontend/` and `services/stt/`. The first build takes a while.

### 4. Wait for the first start

```bash
docker compose logs -f backend      # wait for "listening"
docker compose ps                   # backend and db show (healthy)
```

On every start the backend container runs three steps in order:

1. **`granary-migrate`** applies pending database migrations, in one transaction under a lock.
2. **`granary-seed`** adds default data that is missing: permissions and roles, the Owner account (with an
   employee record), document number sequences, every system setting with its default, the default approval
   policies, a chart of accounts, the account mapping for automatic ledger posting, and the fiscal periods of the
   current year. It never overwrites values you have edited. It creates **no taxes**.
3. **`granary-api`** starts serving.

If step 1 or 2 fails, the API does not start. Read `docker compose logs backend`. Both steps are safe to run
again.

You don't need to create storage buckets. Granary creates each bucket the first time it stores a file there.

### 5. Sign in as the Owner

The seed creates one superadmin, the **Owner**, from `SUPERADMIN_EMAIL`, `SUPERADMIN_NAME` and
`SUPERADMIN_PASSWORD`. It is created only while no superadmin exists, so changing these values later has no
effect.

1. Open your Granary address (`http://localhost:3000` locally, or your domain once Nginx is set up).
2. Sign in with the email and password from `.env`.
3. Open your profile (**My profile**) and change the password under **Change password**.
4. Turn on **Two-factor sign-in** on the same page (see [Administration](Administration#two-factor-sign-in)).

![The sign-in page](images/installation-login.png)

Then continue with [First Steps](First-Steps).

---

## Reverse proxy and TLS (Nginx)

Nginx runs on the host, not in Docker. The site configuration is `infra/nginx/granary.conf`. It routes:

| Path | Goes to |
|---|---|
| `/` | The frontend (port 3000) |
| `/api/` | The backend (port 8000), with a rate limit per address |
| `/api/v1/auth/login` | The backend, with a stricter rate limit (5 per minute per address, burst 10) |
| `/api/v1/ai/` | The backend, unbuffered with a long timeout (Nary streams its answers) |
| `/api/v1/documents/` | The backend, 180 s timeout (sending a document renders its PDF) |
| `/api/auth/` | The frontend (sign-in sessions) |
| `/.well-known/oauth-*`, `/api/v1/mcp/oauth/` | The backend (browser login for AI clients) |
| `/ws/signal` | The signalling service (port 3003), WebSocket |
| `/minio/` | The storage (port 9000), read-only: links to public files and short signed links to private files |
| `/health` | The backend health check |

### Local network or trial (HTTP)

```bash
sudo apt-get update && sudo apt-get install -y nginx
sudo cp infra/nginx/granary.conf /etc/nginx/sites-available/granary
sudo ln -s /etc/nginx/sites-available/granary /etc/nginx/sites-enabled/granary
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t && sudo systemctl reload nginx
curl http://localhost/health        # {"status":"ok","service":"granary"}
sudo ufw allow 80/tcp
```

### Public domain with Let's Encrypt

For a server with a public IP and ports 80 and 443 open. Point a DNS A record at the server first.

```bash
sudo apt-get install -y nginx certbot python3-certbot-nginx
sudo cp infra/nginx/granary.conf /etc/nginx/sites-available/granary
sudo ln -s /etc/nginx/sites-available/granary /etc/nginx/sites-enabled/granary
sudo rm -f /etc/nginx/sites-enabled/default
sudo sed -i 's/server_name _;/server_name granary.example.com;/' /etc/nginx/sites-available/granary
sudo nginx -t && sudo systemctl reload nginx

sudo certbot --nginx -d granary.example.com \
  --email admin@example.com --agree-tos --no-eff-email --redirect
sudo certbot renew --dry-run
sudo ufw allow 80/tcp 443/tcp
```

Certbot adds the HTTPS server block to the **live** Nginx file. From then on, never copy the repository's
`granary.conf` over the live file: apply changes to it by hand (see
[Backups and Upgrades](Backups-and-Upgrades#nginx-changes)).

Behind a Cloudflare-proxied DNS record nothing extra is needed: `granary.conf` trusts the `CF-Connecting-IP`
header only from Cloudflare's published address ranges, so rate limits and the addresses Granary records are
the real visitor's. When Cloudflare publishes new ranges, add them to the `set_real_ip_from` list.

### Public domain through a Cloudflare Tunnel

No port forwarding or static IP is needed. Set up the HTTP option above first, then install `cloudflared`,
create a tunnel (`cloudflared tunnel create granary`), route your hostname to `http://localhost:80` in
`/etc/cloudflared/config.yml`, create the DNS record (`cloudflared tunnel route dns granary granary.example.com`)
and run it as a service (`sudo cloudflared service install`). The repository's `docs/DEPLOYMENT.md` has the full
commands.

### After switching to your domain

Set `APP_URL`, `SIGNAL_URL`, `MINIO_PUBLIC_URL` and `CORS_ORIGINS` to the `https://` domain in `.env`, then:

```bash
docker compose up -d --build frontend   # bakes the URLs into the browser code
docker compose up -d backend worker beat
```

---

## TURN server for video calls (coturn)

Video calls are peer to peer (WebRTC). When two people are behind strict firewalls, their media is relayed
through **coturn**, which runs on the host network with `infra/coturn/turnserver.conf`.

1. Set in `.env`:

   ```dotenv
   TURN_SECRET=<random>                 # shared by coturn and the backend
   TURN_EXTERNAL_IP=<server public IP>
   TURN_URIS=["turn:granary.example.com:3478","turns:granary.example.com:5349"]
   ```

2. Open the firewall:

   ```bash
   sudo ufw allow 3478/tcp 3478/udp 5349/tcp 5349/udp   # TURN/STUN
   sudo ufw allow 49152:65535/udp                        # relay range
   ```

3. Restart: `docker compose up -d coturn backend`.

The shipped coturn configuration refuses to relay to private and loopback addresses, so a call can't be used
to reach your internal network. `TURN_URIS` must be a JSON list.

---

## Building the images on another machine

If the server is too small to compile Rust, build the images on a bigger machine and stream them over:

```bash
# on the build machine, in the repository
docker compose build backend pdf face signal research
docker save granary-rust-api:latest granary-rust-pdf:latest granary-rust-face:latest \
    granary-rust-signal:latest granary-rust-research:latest \
  | gzip | ssh <user>@<server> 'gunzip | docker load'

# on the server: check that all five images are there, then start without --build
docker image inspect granary-rust-api:latest granary-rust-pdf:latest granary-rust-face:latest \
    granary-rust-signal:latest granary-rust-research:latest >/dev/null && docker compose up -d
```

Check the images first: Compose builds an image that is missing, whatever else you ask. The `frontend` and `stt`
images are small enough to build on the server: `docker compose up -d --build frontend stt` (name the services;
a bare `--build` would compile Rust).

---

## WhatsApp (optional)

Granary does not include a WhatsApp Web bridge. The WhatsApp inbox, sending documents over WhatsApp and Nary's
WhatsApp channel talk to an HTTP service at `WHATSAPP_SERVICE_URL` with the token `WHATSAPP_SERVICE_TOKEN`. If
you run such a service, pair it in **Settings > Email & WhatsApp > WhatsApp link**. Without one these features
show as disconnected.

---

## Related pages

- [First Steps](First-Steps)
- [Configuration](Configuration)
- [Backups and Upgrades](Backups-and-Upgrades)
- [Troubleshooting](Troubleshooting)

<!-- nav:bottom -->
---

← Previous: [Granary](Home) | Next: [First Steps](First-Steps) →
<!-- /nav:bottom -->
