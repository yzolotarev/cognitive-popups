#!/bin/sh
# User-local installation only. Never edits compositor config or old archives.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
data=${XDG_DATA_HOME:-"$HOME/.local/share"}
bindir=${XDG_BIN_HOME:-"$HOME/.local/bin"}
payload="$data/cognitive-popups"
dest="$bindir/cognitive-popups"

# The five actions are typed and bound as `cognitive-popups <action>`, so the
# binary has to be reachable by name. A private subdirectory under share/ is not
# on PATH: the first real install did exactly that and every hotkey died with
# "Unknown command".
mkdir -p "$bindir" "$data/applications"
install -Dm755 "$here/cognitive-popups" "$dest"
install -Dm644 "$here/hyprland.conf" "$payload/hyprland.conf"
if [ -f "$here/hyprland.lua" ]; then
    install -Dm644 "$here/hyprland.lua" "$payload/hyprland.lua"
fi

# A quoted desktop Exec path tolerates spaces. Reject characters with special
# desktop-entry expansion semantics rather than generating an unsafe command.
case "$dest" in *'%'*|*'"'*|*'\'*|*'`'*|*'$'*) echo 'Unsupported installation path' >&2; exit 1;; esac
cat > "$data/applications/cognitive-popups.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Cognitive Popups — управление
Comment=Control panel for subjects, contexts, memory, experiment and settings
Exec="$dest" hub
Terminal=false
Categories=Education;
StartupNotify=true
EOF

case ":$PATH:" in
    *":$bindir:"*) ;;
    *) printf 'Warning: %s is not in PATH; add it before binding the hotkeys.\n' "$bindir" >&2 ;;
esac

if ! "$dest" --version >/dev/null 2>&1; then
    echo 'Installed file does not start; check the Slint/Wayland runtime libraries.' >&2
    exit 1
fi

printf 'Installed: %s (%s)\n' "$dest" "$("$dest" --version)"
printf 'No autostart entry was created; the actions run on demand.\n'
printf 'Keybinds are opt-in and never installed automatically:\n'
printf '  Hyprland 0.56 (Lua): %s\n' "$payload/hyprland.lua"
printf '  Hyprland (conf):     %s\n' "$payload/hyprland.conf"
printf 'Remove or disable old Cognitive Popups binds first; see the files.\n'
