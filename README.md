# cliphist-gui & launch-gui

A clipboard manager and app launcher for Wayland. Built with GTK4 and Layer Shell. Lightweight, fast, and fully keyboard-driven with optional vim keybindings.

![Preview](./assets/preview.png)

## What these do

**cliphist-gui** - A visual frontend for [cliphist](https://github.com/sentriz/cliphist). Shows your clipboard history with image thumbnails, lets you search and filter, delete entries, all with keyboard shortcuts. Detects content type automatically (text, images, URLs).

**launch-gui** - An app launcher that reads your `.desktop` files. Has fuzzy search, remembers which apps you use most, and includes a calculator (just type `= 2+2`).

Both run as daemons - they start once and stay in memory, so toggling them is instant.

## Dependencies

# Arch

pacman -S cliphist wl-clipboard gtk4 gtk4-layer-shell imagemagick rust pkg-config

You need:

- [cliphist](https://github.com/sentriz/cliphist) for clipboard history
- [wl-clipboard](https://github.com/bugaevc/wl-clipboard) for copying
- ImageMagick for thumbnails
- A Wayland compositor (I use Hyprland)

## Install

git clone https://github.com/vib1240n/cliphist-gui.git
cd cliphist-gui
./install.sh # builds and installs both
./install.sh cliphist # just cliphist-gui
./install.sh launcher # just launch-gui

Binaries go to `~/.local/bin/`.

## Usage

cliphist-gui # start daemon, or toggle if already running
cliphist-gui toggle # toggle visibility
cliphist-gui --reload # restart after config changes
cliphist-gui --help # see all options

Same for `launch-gui`.

## Config

Default config lives at `~/.config/cliphist-gui/config` (and `~/.config/launch-gui/config`).

Generate a template:

cliphist-gui --generate-config
launch-gui --generate-config

This creates the config directory with a `config` file and `style.css` you can edit.

## Vim mode

Both tools have optional vim-style keybindings. Enable with `vim_mode = true` in your config.

- `j/k` to move up/down
- `gg` and `G` for top/bottom
- `i` to enter insert mode (search)
- `Esc` to go back to normal mode, or close
- `dd` to delete an entry (cliphist only)

The mode shows in the status bar.

## Themes

Comes with a few built-in themes: catppuccin, dracula, monokai, onedark, material-you, material-3.

cliphist-gui show-themes # list them
cliphist-gui --theme dracula # try one out

Or set `theme = catppuccin` in your config. You can also point it at your own CSS file.

## Hyprland setup

Keybinds:

bind = SUPER, V, exec, cliphist-gui toggle
bind = SUPER, SPACE, exec, launch-gui toggle

For blur:

layerrule = blur, cliphist-gui
layerrule = ignorealpha 0, cliphist-gui
layerrule = blur, launch-gui
layerrule = ignorealpha 0, launch-gui

Make sure cliphist is storing your history:

exec-once = wl-paste --type text --watch cliphist store
exec-once = wl-paste --type image --watch cliphist store

## Logs

Logs go to `~/.local/state/cliphist-gui/` and `~/.local/state/launch-gui/`. They rotate at 10MB.

## Why I made this

I wanted a clipboard manager and launcher that looked good, stayed out of my way, and didn't eat resources. Tried rofi and others but couldn't style them the way I wanted. Also wanted to learn Rust.

## License

MIT
