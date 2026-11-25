#!/usr/bin/env bash
# Container side, as root: starts the system services, installs the files that
# live under /usr, then runs the session as an unprivileged user.
# Usage: entrypoint.sh <desktop>
set -uo pipefail

mkdir -p /run/dbus
dbus-uuidgen --ensure=/etc/machine-id
dbus-daemon --system --fork

# Desktop shells expect logind; a mock with one session is enough.
/usr/bin/python3 -m dbusmock --system --template logind >/tmp/logind.log 2>&1 &
for _ in $(seq 50); do
    dbus-send --system --print-reply --dest=org.freedesktop.login1 /org/freedesktop/login1 \
        org.freedesktop.DBus.Mock.AddSession string:c1 string:seat0 uint32:1000 string:tester boolean:true \
        >/dev/null 2>&1 && break
    sleep 0.2
done

# The tray shells read the translations from this fixed path.
install -Dm644 -t /usr/share/linux-arctis-manager/lang /repo/src/linux_arctis_manager/lang/*.ini

install -d -o tester -g tester -m 700 /run/user/tester
chown tester /out

runuser -u tester -- env \
    HOME=/home/tester XDG_RUNTIME_DIR=/run/user/tester XDG_SESSION_ID=c1 \
    PYTHONPATH=/repo/src PYTHONDONTWRITEBYTECODE=1 \
    XEPHYR_DISPLAY="${XEPHYR_DISPLAY:-}" \
    dbus-run-session -- /repo/tests/desktop/common/session.sh "$@" 2>&1 | tee /tmp/run.log
status=${PIPESTATUS[0]}

cp /tmp/run.log /out/session.log
[[ -n ${OUT_OWNER:-} ]] && chown -R "$OUT_OWNER" /out
exit "$status"
