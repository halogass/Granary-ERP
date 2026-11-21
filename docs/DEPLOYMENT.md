# Deployment

Three ways to expose Granary are covered, plus [upgrading an existing install](#upgrading-an-existing-install)
and the steps [before enabling GL auto-posting](#before-enabling-gl-auto-posting):

- [Option A: local or LAN (HTTP)](#option-a-local-or-lan-http)
- [Option B: public domain through a Cloudflare Tunnel](#option-b-public-domain-through-a-cloudflare-tunnel)
- [Option C: public domain with Let's Encrypt](#option-c-public-domain-with-lets-encrypt)

All three use the same Docker Compose stack. They differ only in the Nginx TLS setup and how the domain reaches
the server.

---

## Prerequisites

### Server

| Resource | Minimum | Recommended |
|---|---|---|
| CPU | 2 cores | 4 cores |
| RAM | 4 GB | 8 GB (the PDF service runs headless Chromium; face matching and speech to text are CPU-heavy) |
| Disk | 20 GB | 50 GB SSD |
| OS | A current Linux with Docker | Ubuntu 22.04 LTS or later, Debian 12 |

The images are prebuilt for each release, so the server compiles nothing. Only
[building from source](#building-from-source) needs about 8 GB of RAM and 10 GB of disk for the build cache.

### Docker

Install Docker Engine with the Compose plugin (see Docker's documentation for your distribution), then check:

```bash
docker --version          # 24 or later
docker compose version    # v2
```

---

## Environment

```bash
git clone <your-repo-url> granary
cd granary
cp .env.example .env
nano .env
```

Replace every value marked `← CHANGE THIS`. Generate each secret separately with `openssl rand -hex 32`. At
minimum:

```dotenv
APP_SECRET_KEY=<random>
JWT_SECRET_KEY=<random, at least 32 characters>
NEXTAUTH_SECRET=<random>
SIGNAL_TICKET_KEY=<random>
INTERNAL_AUTH_TOKEN=<random>

POSTGRES_PASSWORD=<strong password>
DATABASE_URL=postgresql://granary:<same password>@db:5432/granary
REDIS_PASSWORD=<random>
REDIS_URL=redis://:<same redis password>@redis:6379/0
CELERY_BROKER_URL=redis://:<same redis password>@redis:6379/1
CELERY_RESULT_BACKEND=redis://:<same redis password>@redis:6379/2
MINIO_SECRET_KEY=<strong password>

# Service tokens: random strings used only between containers
PDF_SERVICE_TOKEN=<random>
FACE_SERVICE_TOKEN=<random>
RESEARCH_SERVICE_TOKEN=<random>
STT_SERVICE_TOKEN=<random>
SEARXNG_SECRET=<random>

# The superadmin, created on the first start
SUPERADMIN_EMAIL=you@example.com
SUPERADMIN_NAME=Your Name
SUPERADMIN_PASSWORD=<strong initial password>

# Your URLs
APP_URL=https://granary.example.com
SIGNAL_URL=wss://granary.example.com/ws/signal
MINIO_PUBLIC_URL=https://granary.example.com/minio
CORS_ORIGINS=["https://granary.example.com"]
INTERNAL_API_URL=http://backend:8000   # keep as is (Docker-internal)
API_URL=http://backend:8000            # keep as is (Docker-internal)

# TURN server for video calls
TURN_SECRET=<random>
TURN_EXTERNAL_IP=<server public IP>
TURN_URIS=["turn:granary.example.com:3478","turns:granary.example.com:5349"]

# Text inside the e-signature seal
SIGN_SEAL_NAME="Your Company Ltd"
SIGN_LOCATION=<your city>

# Optional: automatic FX rate sync (free tier at exchangerate-api.com)
# EXCHANGERATE_API_KEY=<key>
```

When the API and the worker start, they refuse to run while `JWT_SECRET_KEY`, `APP_SECRET_KEY` or `NEXTAUTH_SECRET`
still holds a placeholder from `.env.example` (the error says how to generate one). Every other secret that still
holds a placeholder is logged and reported to the Owner.

Business settings (taxes, payroll, approval limits, reminder schedules, the company time zone) are not
environment variables. They are seeded with defaults and edited in **Settings**: see
[Configuration](CONFIGURATION.md#16-business-rules-are-settings).

> `INTERNAL_API_URL` and `API_URL` must point to the Docker-internal backend (`http://backend:8000`), not the
> public domain. The public domain there causes an http to https redirect loop that breaks API calls.

---

## Option A: local or LAN (HTTP)

Serves on port 80, without TLS. For internal networks or a trial.

### 1. Install Nginx

```bash
sudo apt-get update && sudo apt-get install -y nginx
```

### 2. Install the site config

```bash
sudo cp infra/nginx/granary.conf /etc/nginx/sites-available/granary
sudo ln -s /etc/nginx/sites-available/granary /etc/nginx/sites-enabled/granary
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t
```

### 3. Start

```bash
docker compose up -d
```

Compose pulls the prebuilt images for `GRANARY_VERSION` (`ghcr.io/halogass/granary-api`, `-pdf`, `-face`,
`-signal`, `-research`, `-frontend`, `-stt`; x86-64). To compile them yourself, see
[Building from source](#building-from-source).

On every start the backend container runs `granary-migrate` (pending migrations, in one transaction under an
advisory lock), then `granary-seed`, then `granary-api`. If either of the first two fails, the backend does not
start. Both are idempotent.

The seed creates the permissions and roles, the superadmin (with an Owner employee record), the document
sequences, every missing system setting, the default approval policies, the chart of accounts, the GL
account-role mapping and the current year's fiscal periods. It inserts only what is missing and never
overwrites values you have edited. It creates no taxes.

### 4. Wait for the backend

```bash
docker compose logs -f backend      # wait for "listening"
docker compose ps                   # backend and db show (healthy)
```

### 5. Reload Nginx and check

```bash
sudo systemctl reload nginx
curl http://localhost/health
# {"status":"ok","service":"granary"}
```

Open `http://localhost` (or `http://<server-ip>`), sign in with `SUPERADMIN_EMAIL` and `SUPERADMIN_PASSWORD`,
and continue with [Configuration](CONFIGURATION.md).

### 6. Open the firewall

```bash
sudo ufw allow 80/tcp
sudo ufw allow 3478/tcp 3478/udp 5349/tcp 5349/udp   # TURN/STUN (video calls)
sudo ufw allow 49152:65535/udp                        # TURN relay range
```

---

## Option B: public domain through a Cloudflare Tunnel

Exposes Granary through a Cloudflare Tunnel: no port forwarding and no static IP needed.

### 1. Do Option A, steps 1 to 5

Get the stack running on HTTP locally first.

### 2. Install cloudflared

```bash
curl -fsSL https://pkg.cloudflare.com/cloudflare-main.gpg \
  | sudo gpg --dearmor -o /usr/share/keyrings/cloudflare-main.gpg
echo "deb [signed-by=/usr/share/keyrings/cloudflare-main.gpg] \
  https://pkg.cloudflare.com/cloudflared any main" \
  | sudo tee /etc/apt/sources.list.d/cloudflared.list
sudo apt-get update && sudo apt-get install -y cloudflared
```

### 3. Create a tunnel

```bash
cloudflared tunnel login
cloudflared tunnel create granary     # note the tunnel ID
```

### 4. Configure the tunnel

```bash
sudo mkdir -p /etc/cloudflared
sudo nano /etc/cloudflared/config.yml
```

```yaml
tunnel: <tunnel-id>
credentials-file: /root/.cloudflared/<tunnel-id>.json

ingress:
  - hostname: granary.example.com
    service: http://localhost:80
  - service: http_status:404
```

### 5. Create the DNS record

```bash
cloudflared tunnel route dns granary granary.example.com
```

### 6. Update `.env` and restart

```bash
# In .env: APP_URL, SIGNAL_URL, MINIO_PUBLIC_URL and CORS_ORIGINS on https://granary.example.com
docker compose up -d backend worker beat frontend
```

### 7. Run the tunnel as a service

```bash
sudo cloudflared service install
sudo systemctl enable --now cloudflared
cloudflared tunnel info granary     # check it is healthy
```

---

## Option C: public domain with Let's Encrypt

For a server with a static public IP and ports 80 and 443 open.

### 1. Point a DNS A record at the server, then install Nginx and Certbot

```bash
sudo apt-get install -y nginx certbot python3-certbot-nginx

sudo cp infra/nginx/granary.conf /etc/nginx/sites-available/granary
sudo ln -s /etc/nginx/sites-available/granary /etc/nginx/sites-enabled/granary
sudo rm -f /etc/nginx/sites-enabled/default
sudo sed -i 's/server_name _;/server_name granary.example.com;/' /etc/nginx/sites-available/granary
sudo nginx -t && sudo systemctl reload nginx
```

### 2. Start the stack

```bash
docker compose up -d
```

### 3. Get the certificate

```bash
sudo certbot --nginx -d granary.example.com \
  --email you@example.com --agree-tos --no-eff-email --redirect
```

Certbot adds the HTTPS server block and the redirect to the live Nginx file.

**Behind a Cloudflare-proxied DNS record:** nothing extra to do. `granary.conf` trusts `CF-Connecting-IP` only
from Cloudflare's published ranges, so Nginx's rate limits and the client address the backend records (the
e-signature audit trail, sign-in throttling) are the real visitor's. The backend reads only `X-Real-IP` from
Nginx. When Cloudflare publishes new ranges (<https://www.cloudflare.com/ips>), add them to the
`set_real_ip_from` list and reload Nginx.

### 4. Update `.env` and restart

```bash
# In .env: APP_URL, SIGNAL_URL, MINIO_PUBLIC_URL and CORS_ORIGINS on https://granary.example.com
docker compose up -d backend worker beat frontend
```

### 5. Check auto-renewal

```bash
sudo certbot renew --dry-run
```

### 6. Open the firewall

```bash
sudo ufw allow 80/tcp 443/tcp
sudo ufw allow 3478/tcp 3478/udp 5349/tcp 5349/udp
sudo ufw allow 49152:65535/udp
```

---

## Building from source

The prebuilt images cover x86-64. To compile them yourself (another CPU type, or your own changes), on a machine
with about 8 GB of RAM:

```bash
docker compose build
docker compose up -d
```

The build tags its result with the prebuilt names, so Compose runs your build; `docker compose pull` returns to
the published images. To build on a bigger machine and load the images on the server:

```bash
docker compose build backend pdf face signal research frontend stt
docker compose config --images | grep granary- | xargs docker save | gzip | ssh <user>@<server> 'gunzip | docker load'
```

The `api` build stage uses `CARGO_BUILD_JOBS=2` to keep memory low; with more RAM pass
`--build-arg CARGO_BUILD_JOBS=4` (or more). CI builds the published images with `docker-bake.hcl`.

---

## Upgrading an existing install

**Take a database backup first** ([Maintenance → Database backups](MAINTENANCE.md#database-backups)).

```bash
git fetch --tags && git checkout v1.0.2  # the new release; set GRANARY_VERSION in .env to match
docker compose pull                       # or `docker compose build` if you build from source
docker compose up -d
docker compose logs -f backend            # watch for errors until "listening"
docker compose exec -T backend granary-migrate --status | tail -3   # newest migration applied
```

The backend, worker and beat share one image, so they always run the same code. Only the backend runs
migrations. If a migration aborts, the backend exits and restarts, and nothing after it is applied: read the
error in `docker compose logs backend`, fix the data it names, and `docker compose restart backend`.

---

## Before enabling GL auto-posting

Automatic posting is **off** until `finance.gl_autopost_from` is set. Before setting it:

1. **Map every role** in **Finance → Chart of Accounts → Account mapping**. The defaults point to the seeded
   chart; check each against your own chart, including the `expense.<category>` roles and `customer_advances`
   (proforma payments are held there until the invoice is issued).
2. **Link bank accounts to GL accounts.** Each bank account used for payments needs its GL account: set it on
   the bank account in **Finance → Bank** (create or edit).
3. **Set depreciation accounts on every asset category** (Finance → Assets → Categories). Depreciation refuses
   to post without them.
4. **Check the payroll settings** (`payroll.contributions`, `payroll.income_tax`) so payroll postings carry the
   right contributions and income tax.
5. **Set the cut-over date** `finance.gl_autopost_from` (Settings → Money & tax → Accounting, `YYYY-MM-DD`).
   From then on, documents dated on or after it post themselves when their event happens (sent, recorded,
   approved, paid). A locked fiscal period refuses postings.
6. **Post opening entries** for earlier documents, once: `POST /api/v1/finance/accounting/migrate-opening`
   (permission `finance:post_journal`). With a cut-over set it takes only documents dated before it, and it skips
   documents that already have an entry.

---

## Managing the stack

```bash
docker compose down                                  # stop everything (volumes are kept)
docker compose up -d --force-recreate frontend        # restart one service
docker compose up -d --force-recreate worker beat    # restart the worker and beat

docker compose logs -f backend
docker compose logs -f frontend
docker compose logs -f worker

sudo nginx -t && sudo systemctl reload nginx
sudo tail -f /var/log/nginx/error.log

docker stats
docker system df -v
```
