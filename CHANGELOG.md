# Changelog

All notable changes to nudu are listed here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-10-17

First release.

### Scanning
- Parallel scan on all CPU cores with a live progress screen; `Esc` cancels, `-t` limits threads
- Apparent size and real space on disk; hard links counted once; sparse files stay sparse
- Stays on one filesystem by default (`--cross-fs` to include other mounts)
- btrfs subvolumes and ZFS datasets of one pool are treated as one filesystem
- `/proc`, `/sys` and other kernel filesystems, and btrfs snapshot folders, are never counted
- On FUSE drives that report no disk usage (sshfs, rclone), sizes are estimated and marked `≈`

### Browsing
- Dashboard layout: breadcrumb and filesystem gauge, file list with bars, details pane, key hints
- Sort by size, name, item count or last change, in either direction
- Details pane with space saved by compression or sparse files, share of the folder,
  biggest items inside, and breakdowns by file type and by age
- Age heat (`a`) and a cold view (`A`) for data untouched for a year or six months
- Owners: package (pacman, dpkg), git repo, Flatpak, Steam game, Docker, app cache/config/data,
  and system files no package claims (`unowned`); `o` shows tags, `O` ranks owners
- Colors follow the terminal's palette; `--no-icons` for terminals without a Nerd Font

### Drives
- Drives list (`D`) with type and usage; `Enter` scans a mounted drive on demand
- Notices drives being mounted or unmounted while open; `g` opens a new drive, `r` rescans
- Steps out automatically when the open folder's drive is unmounted

### Deleting
- `d` asks for confirmation; only `y` deletes
- Removes everything it can and reports what it could not; totals update without a rescan
- Never enters a mounted drive inside the folder being deleted
- Warns before deleting files a package installed; `-r` turns deleting off

### Root
- `install.sh` installs to `/usr/local/bin` so `sudo nudu` works
- `S` restarts nudu with sudo when folders were unreadable; a `root` badge shows root mode

[0.1.0]: https://github.com/qempexe/nudu/releases/tag/v0.1.0
