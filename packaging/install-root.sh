#!/usr/bin/env bash
# DaemonHall, root side. Run once with sudo from the checkout, after
# ./packaging/install.sh (which builds the helper):
#
#   - installs daemonhall-unitctl root-owned at /usr/local/bin
#   - installs its polkit action (admin password by default)
#   - writes /etc/polkit-1/rules.d/50-daemonhall.rules, which lets YOU (the
#     sudo user) run the helper and start/stop/restart only the units on
#     your DaemonHall/cyberwatch watch list without a password.
#
# Undo: sudo ./packaging/install-root.sh --uninstall
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $EUID -eq 0 ]] || { echo "Run with sudo." >&2; exit 1; }
[[ -n "${SUDO_UID:-}" && "${SUDO_UID}" != 0 ]] || { echo "Run with sudo from your own account (SUDO_UID is how it knows whose units)." >&2; exit 1; }

if [[ "${1:-}" == "--uninstall" ]]; then
  rm -f /usr/local/bin/daemonhall-unitctl \
        /usr/share/polkit-1/actions/tech.cybercore.daemonhall.unitctl.policy \
        /etc/polkit-1/rules.d/50-daemonhall.rules
  echo "Removed the DaemonHall helper and polkit files (backups in /var/lib/daemonhall kept)."
  exit 0
fi

user_home="$(getent passwd "$SUDO_UID" | cut -d: -f6)"
# Built and staged by ./packaging/install.sh (run as your user first).
helper="$user_home/.local/share/daemonhall/daemonhall-unitctl"
[[ -f "$helper" && ! -L "$helper" ]] || { echo "Run ./packaging/install.sh as your user first (it builds and stages the helper)." >&2; exit 1; }

install -o root -g root -m 0755 "$helper" /usr/local/bin/daemonhall-unitctl
install -o root -g root -m 0644 packaging/tech.cybercore.daemonhall.unitctl.policy \
  /usr/share/polkit-1/actions/tech.cybercore.daemonhall.unitctl.policy
install -d -o root -g root -m 0755 /etc/polkit-1/rules.d /var/lib/daemonhall
/usr/local/bin/daemonhall-unitctl sync
echo
echo "Done. Polkit rule: /etc/polkit-1/rules.d/50-daemonhall.rules"
echo "It's regenerated whenever you change the watch list in DaemonHall."
