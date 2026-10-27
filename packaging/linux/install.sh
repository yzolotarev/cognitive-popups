#!/bin/sh
# User-local installation only. Never edits compositor config or old archives.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
data=${XDG_DATA_HOME:-"$HOME/.local/share"}
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
Name=Cognitive Popups — управление
Comment=Control panel for subjects, sessions, memory, experiment and settings
Exec="$dest" hub
Terminal=false
Categories=Education;
StartupNotify=true
EOF
if [ "${1:-}" = '--autostart' ]; then
    printf 'Nothing to autostart: every action is a one-shot command bound to a hotkey.\n' >&2
fi
printf 'Installed: %s\nNo autostart entry was created; the actions run on demand.\n' "$dest"
printf 'Hyprland keybinds are an opt-in example: %s\n' "$data/cognitive-popups/hyprland.conf"
printf 'It is NOT sourced automatically; remove old Alt+E/W/T/I binds first.\n'
