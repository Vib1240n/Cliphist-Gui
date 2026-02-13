#!/bin/bash

# Determine what to build/install
case "${1:-all}" in
    launcher|launch)
        PACKAGES="-p launch-gui"
        BINS="launch-gui"
        ;;
    cliphist|clip)
        PACKAGES="-p cliphist-gui"
        BINS="cliphist-gui"
        ;;
    all|"")
        PACKAGES="-p cliphist-gui -p launch-gui"
        BINS="cliphist-gui launch-gui"
        ;;
    *)
        echo "Usage: $0 [launcher|cliphist|all]"
        exit 1
        ;;
esac

# 1. Build
echo "Building: $BINS"
if cargo build --release $PACKAGES; then
    echo "Build successful."
else
    echo "Build failed."
    exit 1
fi

# 2. Kill & install each binary
for bin in $BINS; do
    echo "Killing $bin"
    pkill "$bin" 2>/dev/null
done

sleep 0.5

# 3. Copy binaries
echo "Copying binaries to ~/.local/bin/"
for bin in $BINS; do
    cp "target/release/$bin" ~/.local/bin/
done

# 4. Restart daemons
echo "Starting daemons"
for bin in $BINS; do
    ~/.local/bin/"$bin" > /dev/null 2>&1 &
done

echo "Done! Running: $BINS"

