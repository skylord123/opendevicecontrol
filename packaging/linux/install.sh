#!/bin/sh
# Install opendevicecontrol for the current user (binary, icon, menu entry) and the
# system-wide udev rule (needs sudo). Run from the repository root after
# `cargo build --release`, or pass the binary path as $1.
set -eu

BIN="${1:-target/release/opendevicecontrol}"
HERE="$(cd "$(dirname "$0")" && pwd)"

if [ ! -x "$BIN" ]; then
    echo "binary not found: $BIN (run 'cargo build --release' first)" >&2
    exit 1
fi

echo "==> installing binary to ~/.local/bin"
install -Dm755 "$BIN" "$HOME/.local/bin/opendevicecontrol"

echo "==> installing icon and menu entry"
ICON_DIR="$HOME/.local/share/icons/hicolor/256x256/apps"
mkdir -p "$ICON_DIR"
"$HOME/.local/bin/opendevicecontrol" --export-icon "$ICON_DIR/opendevicecontrol.png"
install -Dm644 "$HERE/opendevicecontrol.desktop" "$HOME/.local/share/applications/opendevicecontrol.desktop"
sed -i "s|^Exec=opendevicecontrol|Exec=$HOME/.local/bin/opendevicecontrol|" "$HOME/.local/share/applications/opendevicecontrol.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$HOME/.local/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" || true

echo "==> installing udev rule (sudo)"
sudo install -Dm644 "$HERE/60-opendevicecontrol.rules" /etc/udev/rules.d/60-opendevicecontrol.rules
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=hidraw
sudo udevadm trigger --subsystem-match=leds
if [ -f "$HERE/modules-load.d/opendevicecontrol-i2c.conf" ]; then
    echo "==> loading i2c-dev at boot (RAM lighting, only used if enabled in Settings)"
    sudo install -Dm644 "$HERE/modules-load.d/opendevicecontrol-i2c.conf" /etc/modules-load.d/opendevicecontrol-i2c.conf
    sudo modprobe i2c-dev || true
    sudo udevadm trigger --subsystem-match=i2c-dev || true
fi

echo
echo "Done. Start opendevicecontrol from the app menu, then enable"
echo "'Start opendevicecontrol when I log in' in Settings for autostart."
case ":$PATH:" in
    *":$HOME/.local/bin:"*) ;;
    *) echo "Note: ~/.local/bin is not on your PATH." ;;
esac
