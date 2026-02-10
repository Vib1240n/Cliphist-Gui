#!/usr/bin/env bash
# Build and install cliphist-gui
# Dependencies: gtk4, gtk4-layer-shell, rust/cargo, cliphist, wl-clipboard, imagemagick

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "-- Checking dependencies..."
for cmd in cargo cliphist wl-copy magick; do
    if ! command -v "$cmd" &>/dev/null; then
        echo "ERROR: $cmd not found. Please install it first."
        exit 1
    fi
done

# Check GTK4 and layer shell dev packages
if ! pkg-config --exists gtk4 2>/dev/null; then
    echo "ERROR: gtk4 dev package not found."
    echo "  Arch: sudo pacman -S gtk4"
    exit 1
fi

if ! pkg-config --exists gtk4-layer-shell-0 2>/dev/null; then
    echo "ERROR: gtk4-layer-shell not found."
    echo "  Arch: sudo pacman -S gtk4-layer-shell"
    exit 1
fi

echo "-- Building release binary..."
cargo build --release

BINARY="target/release/cliphist-gui"
if [[ ! -f "$BINARY" ]]; then
    echo "ERROR: Build failed, binary not found."
    exit 1
fi

SIZE=$(du -h "$BINARY" | cut -f1)
echo "-- Binary size: $SIZE"

echo "-- Installing to ~/.local/bin/"
mkdir -p ~/.local/bin
cp "$BINARY" ~/.local/bin/cliphist-gui
chmod +x ~/.local/bin/cliphist-gui

echo "-- Done. Binary installed to ~/.local/bin/cliphist-gui"
echo ""
echo "Add to your Hyprland config:"
echo "  bind = HYPER, V, exec, cliphist-gui"
echo ""
echo "Add window rules:"
echo "  windowrulev2 = float, class:(com.vib1240n.cliphist-gui)"
echo "  windowrulev2 = pin, class:(com.vib1240n.cliphist-gui)"
echo "  windowrulev2 = stayfocused, class:(com.vib1240n.cliphist-gui)"
