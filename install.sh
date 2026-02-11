#!/bin/bash

# 1. Build the project
echo "Starting Install and Building using Cargo"
if cargo build --release -p cliphist-gui -p launch-gui; then
    echo "Build successful."
else
    echo "Build failed."
    exit 1
fi

# 2. Kill existing processes
echo "Killing existing processes"
pkill cliphist-gui
echo "Cliphist-gui killed"
pkill launch-gui
echo "launch-gui killed"
# Short sleep to ensure file handles are released
sleep 0.5

# 3. Copy binaries to local bin
echo "copying binaries to location ~/.local/bin/"
cp target/release/launch-gui ~/.local/bin/
cp target/release/cliphist-gui ~/.local/bin/

# 4. Restart daemons in the background
echo "Starting new proccesses"
~/.local/bin/cliphist-gui > /dev/null 2>&1 &
~/.local/bin/launch-gui > /dev/null 2>&1 &

echo "Done! cliphist-gui and launch-gui are running."
