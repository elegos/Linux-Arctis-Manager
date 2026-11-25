#!/usr/bin/env bash
# Runs the desktop tests in containers, one desktop after the other, and opens
# the screenshot report. Usage: tests/desktop/run.sh [options] <desktop>...|all
set -euo pipefail

here=$(dirname "$0")
repo=$here/../..
engine=${CONTAINER_ENGINE:-$(command -v podman >/dev/null && echo podman || echo docker)}
image=lam-desktop-test
render_node=/dev/dri/renderD128

usage() {
    echo "Usage: $0 [--no-open] [--xephyr] <desktop>...|all"
    echo "Desktops: $(ls "$here/desktops" | sed 's/\.sh$//' | tr '\n' ' ')"
    echo "  --no-open  do not open the report"
    echo "  --xephyr   show X11 desktops in a Xephyr window instead of running headless"
}

open_report=1
xephyr=0
while [[ ${1:-} == --* ]]; do
    case $1 in
        --no-open) open_report=0 ;;
        --xephyr) xephyr=1 ;;
        *) usage >&2; exit 2 ;;
    esac
    shift
done
if [[ ${1:-} == all ]]; then
    mapfile -t desktops < <(ls "$here/desktops" | sed 's/\.sh$//')
else
    desktops=("$@")
fi
((${#desktops[@]})) || { usage >&2; exit 2; }
for desktop in "${desktops[@]}"; do
    [[ -f $here/desktops/$desktop.sh ]] || { usage >&2; exit 2; }
done

results=$repo/dist/desktop-tests

# Docker needs the results handed back explicitly; rootless podman already maps
# the container's root to the host user.
owner=
[[ $engine == *docker* ]] && owner=$(id -u):$(id -g)

build_args=()
[[ -n ${BASE_IMAGE:-} ]] && build_args+=(--build-arg "BASE_IMAGE=$BASE_IMAGE")
"$engine" build "${build_args[@]}" -f "$here/Dockerfile" -t "$image" "$repo"

xephyr_args=()
if ((xephyr)); then
    command -v Xephyr >/dev/null || { echo "--xephyr needs Xephyr installed" >&2; exit 2; }
    number=5
    while [[ -e /tmp/.X11-unix/X$number ]]; do number=$((number + 1)); done
    # -ac: no authentication on this nested display only, for the container.
    Xephyr ":$number" -screen 1280x720 -ac -nolisten tcp &
    xephyr_pid=$!
    trap 'kill "$xephyr_pid" 2>/dev/null || true' EXIT
    until [[ -e /tmp/.X11-unix/X$number ]]; do
        kill -0 "$xephyr_pid" || exit 1
        sleep 0.2
    done
    xephyr_args=(-v "/tmp/.X11-unix/X$number:/tmp/.X11-unix/X$number" -e "XEPHYR_DISPLAY=:$number")
fi

device_args=()
[[ -e $render_node ]] && device_args=(--device "$render_node")

run_desktop() {
    local desktop=$1 out status=0
    out=$results/$desktop/$(date +%Y%m%d-%H%M%S)
    mkdir -p "$out"
    "$engine" run --rm --init --shm-size=512m --security-opt label=disable \
        "${device_args[@]}" "${xephyr_args[@]}" \
        -v "$(realpath "$repo"):/repo:ro" \
        -v "$(realpath "$out"):/out" \
        -e "OUT_OWNER=$owner" \
        "$image" /repo/tests/desktop/common/entrypoint.sh "$desktop" || status=$?
    python3 "$here/report.py" "$out" "$desktop" "$status" || return
    return "$status"
}

failed=0
for desktop in "${desktops[@]}"; do
    run_desktop "$desktop" || failed=1
done

report=$results/index.html
if [[ -f $report ]]; then
    echo "Report: $report"
    if ((open_report)) && command -v xdg-open >/dev/null; then
        xdg-open "$report" >/dev/null 2>&1 &
    fi
fi
exit "$failed"
