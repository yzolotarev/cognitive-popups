#!/bin/sh
# User-local installation only. Never edits compositor config or old archives.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
data=${XDG_DATA_HOME:-"$HOME/.local/share"}
config=${XDG_CONFIG_HOME:-"$HOME/.config"}
dest="$data/cognitive-exoskeleton-v1/bin/cognitive-exoskeleton"
install -Dm755 "$here/cognitive-exoskeleton" "$dest"
mkdir -p "$data/applications"
# A quoted desktop Exec path tolerates spaces. Reject characters with special
# desktop-entry expansion semantics rather than generating an unsafe command.
case "$dest" in *'%'*|*'"'*|*'\'*|*'`'*|*'$'*) echo 'Unsupported installation path' >&2; exit 1;; esac
cat > "$data/applications/cognitive-exoskeleton-v1.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Cognitive Exoskeleton
Comment=Personal learning goals, contextual thinking and evidence
Exec="$dest" hub
Terminal=false
Categories=Education;
StartupNotify=true
EOF
if [ "${1:-}" = '--autostart' ]; then
    mkdir -p "$config/autostart"
    cp "$data/applications/cognitive-exoskeleton-v1.desktop" "$config/autostart/cognitive-exoskeleton-v1.desktop"
fi
printf 'Installed: %s\nAutostart is off unless --autostart was passed.\n' "$dest"
