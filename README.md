# bandcamp-tui

A terminal client for bandcamp.com.

Sections: your collection and wishlist, Discover, the fan feed, search, and
[Bandcamp Daily](https://daily.bandcamp.com) — browse the editorial articles by
section, read them in the terminal, and play or queue the tracks they feature
(straight from the page, as the site's own players do).

Logging in is optional. Discover, search, Daily, artist and album pages and playback
work without an account; log in (press `L` and paste the browser `identity` cookie)
to see your collection, wishlist and feed and to follow or wishlist things. Start with
`--no-login` to skip reading the stored session (the OS keyring is left alone).

## Install

Prebuilt binaries for Linux (x86_64, aarch64) and macOS (Intel, Apple Silicon) are
attached to every [GitHub release](https://github.com/wolf4ood/bandcamp-tui/releases).
The installer puts `bandcamp-tui` in `~/.cargo/bin` (or `~/.bandcamp-tui/bin` when
there is no Cargo installation):

```
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/wolf4ood/bandcamp-tui/releases/latest/download/bandcamp-tui-installer.sh | sh
```

Or build from source with a Rust toolchain:

```
cargo install --git https://github.com/wolf4ood/bandcamp-tui
```

On Linux the binary links against ALSA (`libasound.so.2`), which every desktop
distribution ships; building from source additionally needs the headers
(`libasound2-dev` on Debian/Ubuntu, `alsa-lib-devel` on Fedora, `alsa-lib` on Arch).

## MPRIS

On Linux the player registers as `org.mpris.MediaPlayer2.bandcamp_tui` on the D-Bus
session bus, so desktop media controls, media keys and tools like `playerctl` can show
the current track and control playback:

```
playerctl -p bandcamp_tui metadata
playerctl -p bandcamp_tui play-pause
```

Set `mpris = false` in the config file to turn it off.

## Releasing

Releases are built by [cargo-dist](https://github.com/axodotdev/cargo-dist) from
`.github/workflows/release.yml` (generated, do not edit by hand; run `dist generate`
after changing `dist-workspace.toml`). To cut one:

1. Bump `version` in `Cargo.toml` and commit.
2. Tag with the same version and push: `git tag v0.1.0 && git push origin main v0.1.0`.

The workflow builds all targets, uploads the archives, checksums and installer to a
GitHub release, and publishes it.
