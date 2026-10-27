#!/bin/sh
# User-local installation only. Never edits compositor config or old archives.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
data=${XDG_DATA_HOME:-"$HOME/.local/share"}
config=${XDG_CONFIG_HOME:-"$HOME/.config"}
dest="$data/cognitive-popups/bin/cognitive-popups"
install -Dm755 "$here/cognitive-popups" "$dest"
install -Dm644 "$here/hyprland.conf" "$data/cognitive-popups/hyprland.conf"
mkdir -p "$data/applications"
# A quoted desktop Exec path tolerates spaces. Reject characters with special
# desktop-entry expansion semantics rather than generating an unsafe command.
case "$dest" in *'%'*|*'"'*|*'\'*|*'`'*|*'$'*) echo 'Unsupported installation path' >&2; exit 1;; esac
cat > "$data/applications/cognitive-popups.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Cognitive Popups
Comment=Personal learning goals, contextual thinking and evidence
Exec="$dest" hub
Terminal=false
Categories=Education;
StartupNotify=true
EOF
if [ "${1:-}" = '--autostart' ]; then
    mkdir -p "$config/autostart"
    cp "$data/applications/cognitive-popups.desktop" "$config/autostart/cognitive-popups.desktop"
fi
printf 'Installed: %s\nAutostart is off unless --autostart was passed.\n' "$dest"
printf 'Hyprland keybinds are an opt-in example: %s\n' "$data/cognitive-popups/hyprland.conf"
printf 'It is NOT sourced automatically; remove old Alt+E/W/T/I binds first.\n'
