# space

Workspace set manager for Sway. Organize collections of workspaces into named sets, and switch between them

## Concepts

**Workspace naming**: `<key>(<set>)` — e.g., `Q(main)`, `W(work)`, `1(project)`. Any key prefix works; ice/thaw finds all workspaces ending in `(<set>)`.

**Note on parentheses**: Set matching reads from the end, so `foo(bar)(main)` belongs to set `main` with key `foo(bar)`. Avoid set names containing parentheses.

**Sets**: Named contexts. Each monitor shows its own set, and a set is shown on at most one monitor. Picking a set that another monitor shows swaps the two monitors' sets; picking any other set ices the monitor's old set and thaws the new one. A set reopens where you left it, falling back to `A(<set>)` on first visit. A monitor seen for the first time gets the first known set no other monitor shows.

**Monitors** are identified by `make model serial`, as in `swaymsg -t get_outputs` (and kanshi), so a monitor keeps its set when plugged into a different port.

**Ice/Thaw**: Freezing a set saves window tree structure and moves windows to a parking workspace (`_`). Thawing restores them.

## Build

```bash
cargo build --release
install -D target/release/space ~/.local/bin/space
```

## Commands

| Command | Description |
|---------|-------------|
| `space switch <key>` | Switch to workspace `<key>` in the focused monitor's set |
| `space move <key>` | Move focused window to `<key>` in the focused monitor's set |
| `space set <name>` | Show a set on the focused monitor: swap with the monitor showing it, or ice the old set, thaw the new one and return to its last visited workspace |
| `space set-menu` | Pick the focused monitor's set via wofi |
| `space ice [--set <name>]` | Freeze a set (default: the focused monitor's) |
| `space thaw [--set <name>]` | Restore a set (default: the focused monitor's) |
| `space ice-menu` | Pick frozen set to thaw |
| `space move-to-set` | Pick target set, enter deliver mode |
| `space deliver <key>` | Move all windows from current workspace to `<key>` in target set; if that set is frozen, they are parked and added to its snapshot |
| `space current` | Print the focused monitor's set |
| `space list-iced` | List frozen sets |

## Sway Integration

See [my dotfiles](https://github.com/kzsh/dotfiles/tree/main/linux/config/sway) for an example configuration.

## State Files

Stored in `~/.local/state/space/`:

| File | Purpose |
|------|---------|
| `monitor_sets.json` | Set shown on each monitor, keyed by `make model serial` |
| `space_sets` | Known set names |
| `ice/<name>.json` | Frozen set snapshots |
| `pending_move_target` | Target set for deliver mode |

## Dependencies

- Sway (IPC)
- wofi (optional, for `set-menu`, `ice-menu`, `move-to-set`)
- notify-send (optional, for notifications)

## Documentation

- [Wofi menu setup](docs/wofi-setup.md)
