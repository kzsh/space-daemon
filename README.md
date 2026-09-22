# space

Workspace set manager for Sway. Organizes workspaces into named sets (contexts) with ice/thaw support for hiding and restoring window arrangements.

## Concepts

**Workspace naming**: `<key>(<set>)` — e.g., `Q(main)`, `W(work)`. Fifteen keys (Q W E R T A S D F G Z X C V B) × unlimited sets.

**Sets**: Named contexts. Switching sets auto-ices the old set and auto-thaws the new one.

**Ice/Thaw**: Freezing a set saves window tree structure and moves windows to a parking workspace (`_`). Thawing restores them.

## Build

```bash
cargo build --release
cp target/release/space ~/.local/bin/
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
- wofi (menus)
- notify-send (notifications)
