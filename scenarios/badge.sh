#!/bin/sh
# A count of 3 on the Files tile, as an application sets it with the launcher entry signal, once the dock listens.
sleep 3
exec gdbus emit --session --object-path /org/example/Badge --signal com.canonical.Unity.LauncherEntry.Update \
    "'application://cupertino-files.desktop'" "{'count': <int64 3>, 'count-visible': <true>}"
