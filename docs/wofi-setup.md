# Wofi Menu Setup

This guide covers configuring wofi as the menu interface for space commands.

## Menu Commands

Three commands use wofi for interactive selection:

| Command | Purpose |
|---------|---------|
| `space set-menu` | Switch between sets (auto ice/thaw) |
| `space ice-menu` | Pick a frozen set to thaw |
| `space move-to-set` | Pick target set, then deliver windows |

## Sway Keybindings

Bind the menu commands to keys. Example using right-alt (`Mod3`):

```
set $space space

bindsym $mod3+k exec $space set-menu
bindsym $mod3+Shift+i exec $space ice-menu
bindsym $mod3+n exec $space move-to-set
```

## Deliver Mode

`move-to-set` opens wofi to pick a target set, then enters a sway mode where the next keypress delivers all windows from the current workspace to that set.

Define the deliver mode in sway config:

```
mode "deliver" {
  bindsym q exec $space deliver Q
  bindsym w exec $space deliver W
  bindsym e exec $space deliver E
  bindsym r exec $space deliver R
  bindsym t exec $space deliver T
  bindsym a exec $space deliver A
  bindsym s exec $space deliver S
  bindsym d exec $space deliver D
  bindsym f exec $space deliver F
  bindsym g exec $space deliver G
  bindsym z exec $space deliver Z
  bindsym x exec $space deliver X
  bindsym c exec $space deliver C
  bindsym v exec $space deliver V
  bindsym b exec $space deliver B
  bindsym Escape mode "default"
}
```

Adjust keys to match your workspace convention.

## Wofi Styling

Space passes options to wofi via stdin. Customize appearance in `~/.config/wofi/style.css`. The prompts are:

- "Space set" for `set-menu`
- "Thaw set" for `ice-menu`
- "Move to set" for `move-to-set`

## Complete Example

See [linux/config/sway](https://github.com/kzsh/dotfiles/tree/main/linux/config/sway) for a full working configuration.
