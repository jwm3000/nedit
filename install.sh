#!/usr/bin/env bash
# Baut nEdit im Release-Modus und installiert es für den aktuellen Benutzer.
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release
install -Dm755 target/release/nedit "$HOME/.local/bin/nedit"
install -Dm644 assets/nedit.desktop "$HOME/.local/share/applications/nedit.desktop"
install -Dm644 assets/nedit.svg "$HOME/.local/share/icons/hicolor/scalable/apps/nedit.svg"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
echo "✓ nEdit installiert – starte es über den App-Launcher oder mit 'nedit'."
