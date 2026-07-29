module.exports = {
    apps: [
        {
            name: "ttb",
            script: "./target/release/Time-Tracker-Backend.exe",
            cwd: __dirname,
            interpreter: "none",
            autorestart: true,
            max_restarts: 10,
            min_uptime: "5s",
            env: {
                TIME_TRACKER_ADMIN_PASSWORD: process.env.TIME_TRACKER_ADMIN_PASSWORD,
            },
        },
    ],
};
