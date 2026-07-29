# Admin dashboard setup

Set the dashboard password before starting or restarting the backend:

```powershell
$env:TIME_TRACKER_ADMIN_PASSWORD = "use-a-long-password-here"
```

For a direct backend start:

```powershell
cargo run
```

For PM2, set the environment variable in the same shell and refresh the process environment:

```powershell
pm2 start ecosystem.config.cjs
pm2 restart time-tracker-backend --update-env
```

Open `http://<server-ip>:8765/admin` and sign in. New and existing device
registrations remain inactive until an admin activates them from the Devices
page.
