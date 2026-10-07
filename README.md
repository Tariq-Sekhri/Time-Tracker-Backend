# Time Tracker Backend

Private Rust/SQLite sync server for Time Tracker clients. It includes an admin panel for device approval and log management.

## Run

```powershell
$env:TIME_TRACKER_ADMIN_PASSWORD = "choose-a-strong-password"
cargo run --release
```

The server listens on port `8765`. Open `http://localhost:8765/admin` to sign in.

## Deploy with PM2

```powershell
$env:TIME_TRACKER_ADMIN_PASSWORD = "choose-a-strong-password"
cargo build --release
pm2 start ecosystem.config.cjs
```

After changing the password, run `pm2 restart ttb --update-env`.

## Client flow

1. Client checks `GET /v1/check`, registers at `POST /v1/register`, then waits for approval.
2. Approve the device in **Admin → Devices**.
3. The client syncs with `POST /v1/sync`; logs remain isolated by device UUID.

Data is stored locally in `server.db`. Keep it and the admin password private.
