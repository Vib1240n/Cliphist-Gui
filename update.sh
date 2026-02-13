#!/bin/bash
set -e

REPO="Vib1240n/Cliphist-Gui"
INSTALL_DIR="$HOME/.local/bin"
ARCH="x86_64-linux"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

# Parse arguments
case "${1:-}" in
    cliphist|clip)
        BINS="cliphist-gui"
        ;;
    launcher|launch)
        BINS="launch-gui"
        ;;
    all)
        BINS="cliphist-gui launch-gui"
        ;;
    "")
        echo "What would you like to update?"
        echo "  1) cliphist-gui"
        echo "  2) launch-gui"
        echo "  3) both"
        read -p "Choice [1/2/3]: " choice
        case "$choice" in
            1) BINS="cliphist-gui" ;;
            2) BINS="launch-gui" ;;
            3) BINS="cliphist-gui launch-gui" ;;
            *) error "Invalid choice" ;;
        esac
        ;;
    *)
        echo "Usage: $0 [cliphist|launcher|all]"
        exit 1
        ;;
esac

# Check dependencies
command -v curl >/dev/null 2>&1 || error "curl is required"
command -v jq >/dev/null 2>&1 || error "jq is required"

# Get latest release
info "Checking for updates..."
RELEASE_DATA=$(curl -s "https://api.github.com/repos/$REPO/releases/latest")
TAG=$(echo "$RELEASE_DATA" | jq -r '.tag_name')

if [ "$TAG" = "null" ] || [ -z "$TAG" ]; then
    error "Could not fetch latest release"
fi

info "Latest version: $TAG"

# Download checksums
CHECKSUM_URL="https://github.com/$REPO/releases/download/$TAG/checksums.txt"
CHECKSUMS=$(curl -sL "$CHECKSUM_URL") || error "Failed to download checksums"

UPDATED=0

for bin in $BINS; do
    FILENAME="${bin}-${ARCH}"
    
    # Check if installed
    if [ ! -f "$INSTALL_DIR/$bin" ]; then
        warn "$bin not installed, skipping (use install.sh)"
        continue
    fi
    
    # Compare checksums
    EXPECTED=$(echo "$CHECKSUMS" | grep "$FILENAME" | awk '{print $1}')
    CURRENT=$(sha256sum "$INSTALL_DIR/$bin" | awk '{print $1}')
    
    if [ "$EXPECTED" = "$CURRENT" ]; then
        info "$bin is already up to date"
        continue
    fi
    
    info "Updating $bin..."
    URL="https://github.com/$REPO/releases/download/$TAG/$FILENAME"
    
    curl -sL "$URL" -o "/tmp/$FILENAME" || error "Failed to download $bin"
    
    # Verify checksum
    ACTUAL=$(sha256sum "/tmp/$FILENAME" | awk '{print $1}')
    if [ "$EXPECTED" != "$ACTUAL" ]; then
        error "Checksum mismatch for $bin!"
    fi
    
    # Kill, update, restart
    pkill "$bin" 2>/dev/null || true
    sleep 0.3
    
    mv "/tmp/$FILENAME" "$INSTALL_DIR/$bin"
    chmod +x "$INSTALL_DIR/$bin"
    
    # Restart daemon
    "$INSTALL_DIR/$bin" > /dev/null 2>&1 &
    
    info "$bin updated and restarted"
    UPDATED=$((UPDATED + 1))
done

echo ""
if [ $UPDATED -gt 0 ]; then
    info "Updated $UPDATED binary(ies) to $TAG"
else
    info "Everything is up to date"
fi
