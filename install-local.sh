#!/bin/bash
# Build + install to ~/.local/bin.
#   ./install-local.sh [launcher|cliphist|snippetd|all]
# cliphist-gui / launch-gui are restarted here; launch-snippetd is a systemd
# user unit (systemd/launch-snippetd.service) and is only try-restarted.

case "${1:-all}" in
    launcher|launch) PACKAGES="-p launch-gui"; BINS="launch-gui" ;;
    cliphist|clip)   PACKAGES="-p cliphist-gui"; BINS="cliphist-gui" ;;
    snippetd)        PACKAGES="-p launch-snippetd"; BINS="launch-snippetd" ;;
    all|"")          PACKAGES="-p cliphist-gui -p launch-gui -p launch-snippetd"
                     BINS="cliphist-gui launch-gui launch-snippetd" ;;
    *) echo "Usage: $0 [launcher|cliphist|snippetd|all]"; exit 1 ;;
esac

echo "Building: $BINS"
cargo build --release $PACKAGES || { echo "Build failed."; exit 1; }

for bin in $BINS; do
    [ "$bin" = launch-snippetd ] && continue
    pkill -x "$bin" 2>/dev/null
done
sleep 0.5

echo "Copying binaries to ~/.local/bin/"
mkdir -p ~/.local/bin
for bin in $BINS; do
    cp "target/release/$bin" ~/.local/bin/
done

# user unit for the snippet expander (enable it yourself once ready)
mkdir -p ~/.config/systemd/user
cp systemd/launch-snippetd.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user try-restart launch-snippetd.service 2>/dev/null

echo "Starting daemons"
for bin in $BINS; do
    [ "$bin" = launch-snippetd ] && continue
    setsid -f ~/.local/bin/"$bin" > /dev/null 2>&1
done

echo "Done: $BINS"
