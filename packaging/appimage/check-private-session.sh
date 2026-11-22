#!/bin/bash
# Private session smoke test; generates/reads only supplied QA fixtures.
set -euo pipefail
: "${GNUNE_QA_ROOT:?Set GNUNE_QA_ROOT to an empty owned QA directory}"
: "${GNUNE_QA_APPIMAGE:?Set GNUNE_QA_APPIMAGE to the AppImage under test}"
: "${GNUNE_QA_MEDIA:?Set GNUNE_QA_MEDIA to generated media fixtures}"
mkdir -p "$GNUNE_QA_ROOT"/{home,data,config,cache,runtime,logs}
chmod 700 "$GNUNE_QA_ROOT/runtime"
export HOME="$GNUNE_QA_ROOT/home" XDG_DATA_HOME="$GNUNE_QA_ROOT/data"
export XDG_CONFIG_HOME="$GNUNE_QA_ROOT/config" XDG_CACHE_HOME="$GNUNE_QA_ROOT/cache"
export XDG_RUNTIME_DIR="$GNUNE_QA_ROOT/runtime"
unset DBUS_SESSION_BUS_ADDRESS DISPLAY WAYLAND_DISPLAY SWAYSOCK
export GTK_A11Y=none NO_AT_BRIDGE=1
export GNUNE_AUDIO_NULL=1 GDK_BACKEND=wayland GSK_RENDERER=cairo
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
export APPIMAGE_EXTRACT_AND_RUN=1
cat > "$GNUNE_QA_ROOT/sway.conf" <<'CONFIG'
output HEADLESS-1 mode 1280x800
seat seat0 fallback true
input * xkb_layout us
focus_follows_mouse no
CONFIG
exec dbus-run-session -- bash -euo pipefail <<'SESSION'
sway -c "$GNUNE_QA_ROOT/sway.conf" > "$GNUNE_QA_ROOT/logs/sway.log" 2>&1 &
compositor=$!
app=""
trap 'if [ -n "$app" ]; then kill "$app" 2>/dev/null || :; fi; kill "$compositor" 2>/dev/null || :; wait 2>/dev/null || :' EXIT
for i in $(seq 1 100); do
    socket=$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -type s -name 'sway-ipc.*.sock' -print -quit)
    if [ -n "$socket" ]; then break; fi
    sleep 0.1
done
export SWAYSOCK="$socket" WAYLAND_DISPLAY=wayland-1
for fixture in music.wav music-png.flac music-jpeg.mp3 video.mp4 video.webm; do
    "$GNUNE_QA_APPIMAGE" "$GNUNE_QA_MEDIA/$fixture" > "$GNUNE_QA_ROOT/logs/$fixture.log" 2>&1 &
    app=$!
    sleep 7
    kill -0 "$app"
    swaymsg -r -t get_tree > "$GNUNE_QA_ROOT/$fixture-tree.json"
    grim "$GNUNE_QA_ROOT/$fixture.png"
    if [ "$fixture" = video.mp4 ]; then
        wtype -k space
        sleep 1
        grim "$GNUNE_QA_ROOT/video-paused.png"
        wtype -k Right
        sleep 1
        grim "$GNUNE_QA_ROOT/video-seek.png"
        wtype -k space
        swaymsg '[app_id="org.gnune.Player"] floating enable, resize set width 800 height 600' > /dev/null
        sleep 1
        grim "$GNUNE_QA_ROOT/video-resized.png"
    fi
    swaymsg '[app_id="org.gnune.Player"] kill' > /dev/null
    for i in $(seq 1 50); do
        if ! kill -0 "$app" 2>/dev/null; then break; fi
        sleep 0.1
    done
    wait "$app"
    app=""
done
cp "$XDG_DATA_HOME/gnune/collection.json" "$GNUNE_QA_ROOT/collection-final.json"
SESSION
