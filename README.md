# nudu

A terminal disk usage analyzer that looks and feels like `ncdu`, with a few upgrades:

- **Owners** - see which package, app or repo a file belongs to
- **Filesystem-aware sizes** - correct numbers on btrfs, zfs and friends
- **Age heat** - spot files you haven't touched in ages

> Work in progress.

## Build

```sh
cargo build --release
./target/release/nudu ~/
```
