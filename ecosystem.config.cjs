module.exports = {
    apps: [
        {
            name: "time-tracker-backend",
            script: "./target/release/Time-Tracker-Backend.exe",
            cwd: __dirname,
            interpreter: "none",
            autorestart: true,
            max_restarts: 10,
            min_uptime: "5s",
        },
    ],
};
