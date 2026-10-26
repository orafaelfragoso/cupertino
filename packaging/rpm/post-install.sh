# Creates the login-screen account and reloads unit files. Enables, starts and restarts nothing.
systemd-sysusers cupertino-greeter.conf
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload
fi
