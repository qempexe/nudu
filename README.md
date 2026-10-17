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
 ↑↓ move   ⏎ open   ← back   s n c m sort   ? help   q quit
```

## Features

- **Fast scan** of any folder, staying on one filesystem by default
- **Real sizes**: both apparent size and space actually used on disk, hard links counted once
- **Sorting** by size, name, item count or last change, in either direction
- **Details pane** with type, sizes, share of the folder, item count and last change
- **Filesystem header** showing the filesystem type and how full it is
- **Follows your terminal theme**: all colors come from your terminal's palette

### Planned

- **Owners**: see which package, app or repo a file belongs to
- **Filesystem-aware sizes**: correct numbers on btrfs, zfs and friends
- **Age heat**: spot files you haven't touched in ages

## Install

Requires a recent stable Rust toolchain.

```sh
git clone https://github.com/qempexe/nudu.git
cd nudu
cargo install --path .
```

This puts `nudu` in `~/.cargo/bin`. Make sure that folder is in your `PATH`.

## Usage

```sh
nudu              # current folder
nudu ~            # your home folder
nudu / --cross-fs # everything, including other mounted filesystems
nudu --no-icons   # plain characters if you don't use a Nerd Font
```

| Key | Action |
|---|---|
| `↑` `↓` / `k` `j` | Move |
| `PgUp` `PgDn` / `Home` `End` | Jump |
| `Enter` / `→` / `l` | Open folder |
| `←` / `h` / `<` | Back |
| `s` `n` `c` `m` | Sort by size, name, item count, last modified (again to reverse) |
| `?` | Help |
| `q` / `Esc` | Quit |

Icons use [Nerd Fonts](https://www.nerdfonts.com/). Use `--no-icons` if they show up as boxes.

## License

MIT
