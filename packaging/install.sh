#!/usr/bin/env bash
# DaemonHall, user side: builds, installs ~/.local/bin/daemonhall and the
# daemonhall.service user unit, and starts it on 127.0.0.1:8767.
# The root side (helper + polkit) is separate: sudo ./packaging/install-root.sh
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $EUID -ne 0 ]] || { echo "Run this as your user, not root." >&2; exit 1; }

cargo build --release --locked
target_dir="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import sys,json;print(json.load(sys.stdin)["target_directory"])')"

install -Dm755 "$target_dir/release/daemonhall" "$HOME/.local/bin/daemonhall"
install -Dm644 packaging/daemonhall.service "$HOME/.config/systemd/user/daemonhall.service"
systemctl --user daemon-reload
systemctl --user enable --now daemonhall.service
systemctl --user restart daemonhall.service

echo
echo "DaemonHall is up: http://127.0.0.1:8767/"
if [[ ! -x /usr/local/bin/daemonhall-unitctl ]]; then
  echo "To control system services too, run once:  sudo ./packaging/install-root.sh"
fi
