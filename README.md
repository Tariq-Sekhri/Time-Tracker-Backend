# Time Tracker Backend

**Version 1.0.** Private Rust/SQLite sync server for Time Tracker clients, with an admin dashboard for approving devices.

## Run

Set an admin password, then start the server:

```powershell
$env:TIME_TRACKER_ADMIN_PASSWORD = "choose-a-strong-password"
cargo run --release
```

The server listens on port `8765`; open `http://localhost:8765/admin` to manage devices. For PM2 deployment, run `cargo build --release`, then `pm2 start ecosystem.config.cjs`.

Data is stored in `server.db`. Keep the database and admin password private. See [ADMIN_SETUP.md](ADMIN_SETUP.md) for setup details.
