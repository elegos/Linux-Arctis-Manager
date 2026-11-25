#!/usr/bin/env bash
# Inside the test user's D-Bus session: starts the fake daemon, brings the
# desktop up, then runs the scenarios. Usage: session.sh <desktop>
set -euo pipefail

root=$(dirname "$0")/..
desktop=$1

# shellcheck source=lib.sh
source "$root/common/lib.sh"
# shellcheck source=/dev/null
source "$root/desktops/$desktop.sh"

# The desktop would otherwise keep the output pipe open, and the run hanging,
# after a failure.
trap 'kill $(jobs -p) 2>/dev/null || true' EXIT

install_app_files
python "$root/common/fake_daemon.py" >/out/fake-daemon.log 2>&1 &
wait_for "the fake daemon" bus_name_owned name.giacomofurlan.ArctisManager.Next
de_start

# The same scenarios on every desktop, so the screenshots can be compared.
# Their names are <scenario>--<theme>--<view>.
for theme in $(de_themes); do
    de_apply_theme "$theme"
    log_color_scheme
    for file in "$root"/scenarios/*.sh; do
        scenario=$(basename "$file" .sh)
        log "$theme: $scenario"
        # shellcheck source=/dev/null
        source "$file"
    done
done
log "done"
