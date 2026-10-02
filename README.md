<p align="center"><img src="static/daemon.svg" width="120" alt="DaemonHall daemon"></p>

# DaemonHall

A web control room for **your own systemd services**: every daemon you run, live, in one hall.
Rust (axum), served on `127.0.0.1:8767`, styled with the shared CYBERGRID palette (all 72 themes, plus custom ones).

It's the web big sibling of [cyberwatch](https://github.com/cybercore-tech/cyberwatch) (the Omarchy bar widget + TUI): same watch list, same discovery, same idea of what "needs attention" means.

## What it does

- **The Hall:** every watched unit as a card with state, uptime, CPU, memory and restarts; filter by state or scope; start, stop, restart from the card.
- **Unit pages:** vitals with a CPU sparkline, listening ports, properties and dependencies; **live logs** streamed from the journal (filter, level, pause, follow); per-unit activity; the unit file (edit user units in place, open system units in `sudoedit`).
- **The Chronicle:** a timeline of every start, stop, crash and auto-restart, read from systemd's own journal records, merged with what was done from DaemonHall.
- **The Clockwork:** all timers with next/last run and last result; run any of them now.
- **The Summoning Room:** create a new service (user or system, optional timer) from a form with a live preview of the unit file; hardened by default. Add or remove units from the watch list; delete units (with a backup).
- **Alerts:** desktop notifications when a daemon fails, recovers or flaps (3+ restarts in 10 minutes).

## Install

```bash
git clone https://github.com/cybercore-tech/DaemonHall
cd DaemonHall
./packaging/install.sh            # builds, installs ~/.local/bin/daemonhall + user service
sudo ./packaging/install-root.sh  # optional: lets it control *system* services
```

Then open <http://127.0.0.1:8767/>. Needs Rust, systemd and the [cybercore](https://github.com/cybercore-tech/cybercore) and cyberwatch crates checked out under `~/.sysops` (path dependencies, like the rest of the family).

## Security model

DaemonHall can stop services, so it's built to be boring to attack:

- **Localhost only**, and every request must carry `Host: 127.0.0.1:8767` (or `localhost`), which defeats DNS rebinding.
- **Every state change needs a per-run token** that's embedded in the page and never exposed cross-origin, plus a matching `Origin`. A site open in another tab can't drive it.
- **User units** need no privileges at all.
- **System units** are controlled through polkit, scoped tightly. `install-root.sh` writes a rule that lets *you* (and only you) start/stop/restart the units on your watch list, and only those that are hand-installed regular files in `/etc/systemd/system` (never distro units, never symlinks). Everything else still asks for a password.
- **Creating, enabling, disabling and deleting system units** goes through a small root helper (`daemonhall-unitctl`, via `pkexec`). It re-checks everything itself. It never accepts raw unit text: it renders the unit from validated form fields, forces `User=` to you (so a new unit can't run as root), rejects `ExecStart` prefixes like `+`, escapes `%` specifiers, runs `systemd-analyze verify` before installing, and never writes inside your home directory.
- **Editing a system unit file** is `sudoedit` in a terminal: your password, every time.

## Files

| Path | What |
|---|---|
| `~/.config/cyberwatch/config.json` | the watch list (`extra_units`, `ignore`), shared with cyberwatch |
| `~/.config/daemonhall/settings.json` | notification settings |
| `~/.local/state/daemonhall/activity.jsonl` | what was done from DaemonHall |
| `~/.local/state/daemonhall/backups/`, `/var/lib/daemonhall/backups/` | copies of deleted or edited unit files |
| `/etc/polkit-1/rules.d/50-daemonhall.rules` | the scoped polkit rule (regenerated when the watch list changes) |

## License

MIT, Cybercore Tech.
