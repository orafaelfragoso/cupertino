# After the last version is removed ($1 = 0), reloads unit files. The login-screen account is kept.
if [ "$1" -eq 0 ] && [ -d /run/systemd/system ]; then
    systemctl daemon-reload
fi
