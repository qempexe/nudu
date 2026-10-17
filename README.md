# nudu

**new du** · *Never Underestimate Disk Usage*

A fast terminal disk usage analyzer. Scan a folder, browse it, and see what is eating your disk.

```
 nudu  ~ › .local › share                        btrfs · 410 GiB / 476 GiB ▕████████▋ ▏
╭─ 91.4 GiB · 412 204 items ───────────────────╮╭─ Steam ────────────────────────╮
│ ▸ 󰉋 Steam            62.1 GiB ━━━━━━━━━━  68% ││ Type      Directory            │
│   󰉋 containers       14.0 GiB ━━╸━━━━━━━  15% ││ Size      61.8 GiB             │
│   󰉋 Trash             5.2 GiB ━━━━━━━━━━   6% ││ On disk   62.1 GiB             │
│   󰈔 data.db         412.0 MiB ━━━━━━━━━━  <1% ││ Modified  3 days ago           │
╰───────────────────────────────────────────────╯╰────────────────────────────────╯
 ↑↓ move   ⏎ open   ← back   s n c m sort   d delete   r rescan   D drives   ? help   q quit
```

## Features

- **Fast parallel scan** on all CPU cores, with a live progress screen (`Esc` cancels)
- **Understands filesystems**: btrfs subvolumes (like `/home`) and ZFS datasets of one pool count as one filesystem,
  other disks are labeled and skipped (`--cross-fs` includes them), `/proc`, `/sys` and btrfs snapshots are never double counted
- **Mounted drives**: a drives list (`D`) with type and usage of every disk, see how full a drive is before
  scanning it, `Enter` scans it on demand, and when you plug something in nudu tells you where it was
  mounted (`g` jumps straight there)
- **Real sizes**: both apparent size and space actually used on disk, hard links counted once
- **Sorting** by size, name, item count or last change, in either direction
- **Safe delete**: always asks first, removes what it can, never enters mounted drives, totals update instantly
- **Details pane** with type, sizes, share of the folder, item count and last change
- **Filesystem header** showing the filesystem type and how full it is
- **Follows your terminal theme**: all colors come from your terminal's palette

### Planned

- **Owners**: see which package, app or repo a file belongs to
- **btrfs-aware sizes**: compression savings and space shared with snapshots
- **Age heat**: spot files you haven't touched in ages

## Install

Requires a recent stable Rust toolchain.

```sh
git clone https://github.com/qempexe/nudu.git
cd nudu
./install.sh
```

This builds nudu and installs it to `/usr/local/bin`, so both `nudu` and `sudo nudu` work.
Remove it again with `./install.sh --uninstall`.

Prefer a user-only install? `cargo install --path .` puts it in `~/.cargo/bin` instead
(sudo won't find it there, but nudu can still restart itself as root, see below).

### Folders you can't read

Some folders belong to root or other users. nudu marks them with `!` and keeps going.
Press `S` to rescan with sudo: nudu restarts itself as root with the same options.
A red **root** badge in the header shows when it runs with root rights.

## Usage

```sh
nudu              # current folder
nudu ~            # your home folder
nudu / --cross-fs # everything, including other mounted filesystems
nudu --no-icons   # plain characters if you don't use a Nerd Font
nudu -t 4         # limit scanning to 4 threads
nudu -r           # read-only: deleting is turned off
```

| Key | Action |
|---|---|
| `↑` `↓` / `k` `j` | Move |
| `PgUp` `PgDn` / `Home` `End` | Jump |
| `Enter` / `→` / `l` | Open folder |
| `←` / `h` / `<` | Back |
| `Enter` on a drive | Scan that mounted drive and open it |
| `r` | Rescan the open folder (new files, new mounts) |
| `D` | List all mounted drives, `Enter` opens one |
| `g` | Open the drive that was just mounted |
| `s` `n` `c` `m` | Sort by size, name, item count, last modified (again to reverse) |
| `d` / `Del` | Delete the selected entry (asks first, `y` to confirm) |
| `S` | Rescan with sudo (when some folders were unreadable) |
| `?` | Help |
| `q` / `Esc` | Quit |

Icons use [Nerd Fonts](https://www.nerdfonts.com/). Use `--no-icons` if they show up as boxes.

## License

MIT
