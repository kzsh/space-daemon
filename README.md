# space

Workspace set manager for Sway. Organize collections of workspaces into named sets, and switch between them

## Concepts

**Workspace naming**: `<key>(<set>)` — e.g., `Q(main)`, `W(work)`, `1(project)`. Any key prefix works; ice/thaw finds all workspaces ending in `(<set>)`.

**Note on parentheses**: Set matching reads from the end, so `foo(bar)(main)` belongs to set `main` with key `foo(bar)`. Avoid set names containing parentheses.

**Sets**: Named contexts. Switching sets auto-ices the old set and auto-thaws the new one.

**Ice/Thaw**: Freezing a set saves window tree structure and moves windows to a parking workspace (`_`). Thawing restores them.

## Build

```bash
cargo build --release
install -D target/release/space ~/.local/bin/space
```

## Commands

| Command | Description |
|---------|-------------|
| `space switch <key>` | Switch to workspace `<key>` in current set |
| `space move <key>` | Move focused window to `<key>` in current set |
| `space set <name>` | Change set (ices old, thaws new, switches to `A`) |
| `space set-menu` | Pick set via wofi |
| `space ice [--set <name>]` | Freeze a set |
| `space thaw [--set <name>]` | Restore a set |
| `space ice-menu` | Pick frozen set to thaw |
| `space move-to-set` | Pick target set, enter deliver mode |
| `space deliver <key>` | Move all windows from current workspace to `<key>` in target set |
| `space current` | Print current set |
| `space list-iced` | List frozen sets |

## Sway Integration

See [my dotfiles](https://github.com/kzsh/dotfiles/tree/main/linux/config/sway) for an example configuration.

## State Files

Stored in `~/.local/state/space/`:

| File | Purpose |
|------|---------|
| `current_space_set` | Active set name |
| `space_sets` | Known set names |
| `ice/<name>.json` | Frozen set snapshots |
| `pending_move_target` | Target set for deliver mode |

## Dependencies

- Sway (IPC)
- wofi (optional, for `set-menu`, `ice-menu`, `move-to-set`)
- notify-send (optional, for notifications)

## Documentation

- [Wofi menu setup](docs/wofi-setup.md)
