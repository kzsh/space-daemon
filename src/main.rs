mod assignments;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use swayipc::{Connection, Node, NodeLayout, NodeType};

use crate::assignments::{Assignments, Change};

const PARKING_WORKSPACE: &str = "_";

#[derive(Parser)]
#[command(name = "space", about = "Sway workspace set manager")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Switch to workspace <key> in the focused monitor's set
    Switch { key: char },
    /// Move focused window to workspace <key> in the focused monitor's set
    Move { key: char },
    /// Show a set on the focused monitor (swaps with another monitor showing it)
    Set { name: String },
    /// Pick the focused monitor's set via wofi
    SetMenu,
    /// Freeze a set, moving windows to parking
    Ice {
        #[arg(short, long)]
        set: Option<String>,
    },
    /// Restore a frozen set
    Thaw {
        #[arg(short, long)]
        set: Option<String>,
    },
    /// Pick frozen set to thaw via wofi
    IceMenu,
    /// Pick target set for cross-set move (then use deliver)
    MoveToSet,
    /// Deliver focused window to <key> in pending target set
    Deliver { key: char },
    /// Pin the focused window to workspace <key> (default: the one it is in)
    /// of whichever set is shown
    Pin { key: Option<char> },
    /// Unpin the focused window
    Unpin,
    /// Print the focused monitor's set
    Current,
    /// List frozen sets
    ListIced,
}

/// A window bound to a workspace key rather than to a set: it is carried
/// to `<key>(<set>)` whenever the set on show changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Pin {
    con_id: i64,
    key: char,
}

// Saved tree structure for ice/thaw
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceSnapshot {
    name: String,
    layout: String,
    tree: Vec<TreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum TreeNode {
    #[serde(rename = "window")]
    Window {
        con_id: i64,
        app_id: Option<String>,
        name: Option<String>,
        floating: bool,
    },
    #[serde(rename = "container")]
    Container {
        layout: String,
        children: Vec<TreeNode>,
    },
}

struct State {
    dir: PathBuf,
}

impl State {
    fn new() -> Result<Self> {
        let dir = dirs::state_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))
            .context("no state dir")?
            .join("space");
        Self::at(dir)
    }

    fn at(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(dir.join("ice"))?;
        Ok(Self { dir })
    }

    fn assignments_file(&self) -> PathBuf {
        self.dir.join("monitor_sets.json")
    }

    fn assignments(&self) -> Result<Assignments> {
        let path = self.assignments_file();
        match fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json)
                .with_context(|| format!("failed to parse {}", path.display())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Assignments::default()),
            Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    fn save_assignments(&self, assignments: &Assignments) -> Result<()> {
        fs::write(
            self.assignments_file(),
            serde_json::to_string_pretty(assignments)?,
        )?;
        Ok(())
    }

    fn sets_file(&self) -> PathBuf {
        self.dir.join("space_sets")
    }

    fn list_sets(&self) -> Result<Vec<String>> {
        match fs::read_to_string(self.sets_file()) {
            Ok(s) => Ok(s
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.to_string())
                .collect()),
            Err(_) => Ok(vec!["main".into(), "work".into(), "scratch".into()]),
        }
    }

    fn save_sets(&self, sets: &[String]) -> Result<()> {
        fs::write(self.sets_file(), sets.join("\n") + "\n")?;
        Ok(())
    }

    /// Record a set as the most recently used, so menus offer it first.
    fn touch_set(&self, name: &str) -> Result<()> {
        let mut sets = self.list_sets()?;
        if sets.first().is_some_and(|s| s == name) {
            return Ok(());
        }
        sets.retain(|s| s != name);
        sets.insert(0, name.to_string());
        self.save_sets(&sets)
    }

    fn last_file(&self) -> PathBuf {
        self.dir.join("last_workspace.json")
    }

    fn last_workspaces(&self) -> Result<BTreeMap<String, String>> {
        match fs::read_to_string(self.last_file()) {
            Ok(json) => Ok(serde_json::from_str(&json).unwrap_or_default()),
            Err(_) => Ok(BTreeMap::new()),
        }
    }

    fn save_last_workspaces(&self, last: &BTreeMap<String, String>) -> Result<()> {
        fs::write(self.last_file(), serde_json::to_string_pretty(last)?)?;
        Ok(())
    }

    fn last_workspace(&self, set: &str) -> Result<Option<String>> {
        Ok(self.last_workspaces()?.remove(set))
    }

    /// Record where a set was left, ignoring a workspace of another set or
    /// none at all.
    fn remember_workspace(&self, set: &str, workspace: Option<&str>) -> Result<()> {
        let Some(workspace) = workspace.filter(|w| workspace_belongs_to_set(w, set)) else {
            return Ok(());
        };
        let mut last = self.last_workspaces()?;
        last.insert(set.to_string(), workspace.to_string());
        self.save_last_workspaces(&last)
    }

    fn ice_dir(&self) -> PathBuf {
        self.dir.join("ice")
    }

    fn ice_file(&self, set: &str) -> PathBuf {
        self.ice_dir().join(format!("{}.json", set))
    }

    fn save_ice(&self, set: &str, snapshots: &[WorkspaceSnapshot]) -> Result<()> {
        fs::create_dir_all(self.ice_dir())?;
        let json = serde_json::to_string_pretty(snapshots)?;
        fs::write(self.ice_file(set), json)?;
        Ok(())
    }

    fn load_ice(&self, set: &str) -> Result<Vec<WorkspaceSnapshot>> {
        let json = fs::read_to_string(self.ice_file(set))?;
        Ok(serde_json::from_str(&json)?)
    }

    fn remove_ice(&self, set: &str) -> Result<()> {
        let path = self.ice_file(set);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    fn list_iced(&self) -> Result<Vec<String>> {
        let dir = self.ice_dir();
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut result = vec![];
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if let Some(name) = entry.path().file_stem() {
                result.push(name.to_string_lossy().to_string());
            }
        }
        Ok(result)
    }

    fn pins_file(&self) -> PathBuf {
        self.dir.join("pins.json")
    }

    fn pins(&self) -> Result<Vec<Pin>> {
        match fs::read_to_string(self.pins_file()) {
            Ok(json) => Ok(serde_json::from_str(&json).unwrap_or_default()),
            Err(_) => Ok(vec![]),
        }
    }

    fn save_pins(&self, pins: &[Pin]) -> Result<()> {
        fs::write(self.pins_file(), serde_json::to_string_pretty(pins)?)?;
        Ok(())
    }

    fn pending_target_file(&self) -> PathBuf {
        self.dir.join("pending_move_target")
    }

    fn set_pending_target(&self, set: &str) -> Result<()> {
        fs::write(self.pending_target_file(), set)?;
        Ok(())
    }

    fn get_pending_target(&self) -> Result<Option<String>> {
        match fs::read_to_string(self.pending_target_file()) {
            Ok(s) => Ok(Some(s.trim().to_string())),
            Err(_) => Ok(None),
        }
    }

    fn clear_pending_target(&self) -> Result<()> {
        let path = self.pending_target_file();
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

fn workspace_name(key: char, set: &str) -> String {
    format!("{}({})", key.to_ascii_uppercase(), set)
}

fn layout_to_string(layout: NodeLayout) -> String {
    match layout {
        NodeLayout::SplitH => "splith",
        NodeLayout::SplitV => "splitv",
        NodeLayout::Tabbed => "tabbed",
        NodeLayout::Stacked => "stacking",
        _ => "splith",
    }
    .to_string()
}

/// Extract tree structure from a sway node, preserving hierarchy
fn extract_tree(node: &Node, floating: bool) -> Option<TreeNode> {
    match node.node_type {
        NodeType::Con if node.pid.is_some() => {
            // Leaf window
            Some(TreeNode::Window {
                con_id: node.id,
                app_id: node.app_id.clone(),
                name: node.name.clone(),
                floating,
            })
        }
        NodeType::Con | NodeType::FloatingCon => {
            // Container with children
            let children: Vec<TreeNode> = node
                .nodes
                .iter()
                .filter_map(|n| extract_tree(n, false))
                .collect();

            if children.is_empty() {
                None
            } else if let [TreeNode::Window { .. }] = children.as_slice() {
                // Single window, don't wrap in container
                children.into_iter().next()
            } else {
                Some(TreeNode::Container {
                    layout: layout_to_string(node.layout),
                    children,
                })
            }
        }
        _ => None,
    }
}

/// Find the focused workspace node
fn find_focused_workspace(node: &Node) -> Option<&Node> {
    if node.node_type == NodeType::Workspace && node.focused {
        return Some(node);
    }
    // Check if any child workspace is focused or contains focus
    for child in &node.nodes {
        if let Some(ws) = find_focused_workspace(child) {
            return Some(ws);
        }
    }
    // For workspaces, check if this contains the focused window
    if node.node_type == NodeType::Workspace && has_focused_descendant(node) {
        return Some(node);
    }
    None
}

fn has_focused_descendant(node: &Node) -> bool {
    if node.focused {
        return true;
    }
    for child in &node.nodes {
        if has_focused_descendant(child) {
            return true;
        }
    }
    for child in &node.floating_nodes {
        if has_focused_descendant(child) {
            return true;
        }
    }
    false
}

/// Snapshot a workspace node's contents, or None if it holds no windows.
fn snapshot_workspace(node: &Node) -> Option<WorkspaceSnapshot> {
    let mut tree: Vec<TreeNode> = node
        .nodes
        .iter()
        .filter_map(|n| extract_tree(n, false))
        .collect();
    tree.extend(node.floating_nodes.iter().filter_map(|n| extract_tree(n, true)));

    if tree.is_empty() {
        return None;
    }
    Some(WorkspaceSnapshot {
        name: node.name.clone().unwrap_or_default(),
        layout: layout_to_string(node.layout),
        tree,
    })
}

/// Drop pinned windows from `snapshots`. A pin belongs to whichever set is
/// on show, so freezing one along with the set it happens to be visiting
/// would restore a second copy after the window has moved on.
fn strip_pins(snapshots: &mut Vec<WorkspaceSnapshot>, pinned: &[i64]) {
    fn keep(nodes: Vec<TreeNode>, pinned: &[i64]) -> Vec<TreeNode> {
        nodes
            .into_iter()
            .filter_map(|node| match node {
                TreeNode::Window { con_id, .. } if pinned.contains(&con_id) => None,
                TreeNode::Container { layout, children } => {
                    let children = keep(children, pinned);
                    (!children.is_empty()).then_some(TreeNode::Container { layout, children })
                }
                window => Some(window),
            })
            .collect()
    }

    for snapshot in snapshots.iter_mut() {
        snapshot.tree = keep(std::mem::take(&mut snapshot.tree), pinned);
    }
    snapshots.retain(|snapshot| !snapshot.tree.is_empty());
}

/// Add `tree` to the snapshot of `workspace`, creating it if the frozen set
/// has none. Existing contents keep their layout and come first.
fn splice_snapshot(
    snapshots: &mut Vec<WorkspaceSnapshot>,
    workspace: &str,
    layout: &str,
    tree: Vec<TreeNode>,
) {
    match snapshots.iter_mut().find(|s| s.name == workspace) {
        Some(existing) => existing.tree.extend(tree),
        None => snapshots.push(WorkspaceSnapshot {
            name: workspace.to_string(),
            layout: layout.to_string(),
            tree,
        }),
    }
}

/// Collect window con_ids from tree
fn collect_window_ids(tree: &[TreeNode]) -> Vec<i64> {
    let mut ids = vec![];
    for node in tree {
        match node {
            TreeNode::Window { con_id, .. } => ids.push(*con_id),
            TreeNode::Container { children, .. } => {
                ids.extend(collect_window_ids(children));
            }
        }
    }
    ids
}

/// Restore tree structure in a workspace
fn restore_tree(conn: &mut Connection, tree: &[TreeNode], workspace: &str) -> Result<usize> {
    let mut count = 0;

    for node in tree {
        count += restore_node(conn, node, workspace, true)?;
    }

    Ok(count)
}

fn restore_node(
    conn: &mut Connection,
    node: &TreeNode,
    workspace: &str,
    is_first: bool,
) -> Result<usize> {
    match node {
        TreeNode::Window {
            con_id, floating, ..
        } => {
            let cmd = format!("[con_id={}] move to workspace \"{}\"", con_id, workspace);
            if conn.run_command(&cmd).is_ok() {
                if *floating {
                    let _ = conn.run_command(format!("[con_id={}] floating enable", con_id));
                }
                Ok(1)
            } else {
                Ok(0)
            }
        }
        TreeNode::Container { layout, children } => {
            let mut count = 0;
            let mut first_in_container = true;

            for child in children {
                if !first_in_container && !is_first {
                    // Apply split before adding subsequent windows
                    let split_cmd = match layout.as_str() {
                        "splitv" => "splitv",
                        "tabbed" => "layout tabbed",
                        "stacking" => "layout stacking",
                        _ => "splith",
                    };
                    let _ = conn.run_command(split_cmd);
                }

                count += restore_node(conn, child, workspace, is_first && first_in_container)?;
                first_in_container = false;
            }

            Ok(count)
        }
    }
}

/// The focused window, skipping the containers and workspace above it.
fn focused_window_id(node: &Node) -> Option<i64> {
    if node.focused && node.pid.is_some() {
        return Some(node.id);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(focused_window_id)
}

fn find_workspace<'a>(node: &'a Node, name: &str) -> Option<&'a Node> {
    if node.node_type == NodeType::Workspace && node.name.as_deref() == Some(name) {
        return Some(node);
    }
    node.nodes.iter().find_map(|child| find_workspace(child, name))
}

/// The window in the top-left slot: sway keeps the tiling tree in layout
/// order, so it is the first leaf reached depth-first.
fn first_tiled_window(node: &Node) -> Option<i64> {
    if node.pid.is_some() {
        return Some(node.id);
    }
    node.nodes.iter().find_map(first_tiled_window)
}

fn all_window_ids(node: &Node) -> Vec<i64> {
    let mut ids = vec![];
    if node.pid.is_some() {
        ids.push(node.id);
    }
    for child in node.nodes.iter().chain(node.floating_nodes.iter()) {
        ids.extend(all_window_ids(child));
    }
    ids
}

/// Carry every pinned window to its key in `set`, dropping pins whose
/// window has closed. Pins are placed in reverse order so that the first
/// pinned window of a workspace ends up holding the top-left slot.
fn carry_pins(conn: &mut Connection, state: &State, set: &str) -> Result<()> {
    let mut pins = state.pins()?;
    let live = all_window_ids(&conn.get_tree()?);
    let before = pins.len();
    pins.retain(|pin| live.contains(&pin.con_id));
    if pins.len() != before {
        state.save_pins(&pins)?;
    }

    for pin in pins.iter().rev() {
        place_pin(conn, pin, set)?;
    }
    Ok(())
}

/// Move one pinned window into `set` and swap it into the top-left slot,
/// displacing the window that held it into the slot the pin arrived in.
fn place_pin(conn: &mut Connection, pin: &Pin, set: &str) -> Result<()> {
    let workspace = workspace_name(pin.key, set);
    let resident = find_workspace(&conn.get_tree()?, &workspace)
        .and_then(first_tiled_window)
        .filter(|id| *id != pin.con_id);

    run(
        conn,
        &format!(
            "[con_id={}] move container to workspace \"{}\"",
            pin.con_id, workspace
        ),
    )?;
    if let Some(resident) = resident {
        run(
            conn,
            &format!(
                "[con_id={}] swap container with con_id {}",
                pin.con_id, resident
            ),
        )?;
    }
    Ok(())
}

fn notify(msg: &str) {
    let _ = std::process::Command::new("notify-send").arg(msg).spawn();
}

/// The key a workspace of `set` is reached by, for the single-character
/// keys `switch` and `pin` take. A hand-made name like `foo(bar)(main)`
/// has no such key.
fn workspace_key(ws_name: &str, set: &str) -> Option<char> {
    let prefix = ws_name.strip_suffix(&format!("({})", set))?;
    let mut chars = prefix.chars();
    let key = chars.next()?;
    chars.next().is_none().then_some(key)
}

/// Check if workspace name belongs to a set: matches pattern `*(<set>)`
fn workspace_belongs_to_set(ws_name: &str, set_name: &str) -> bool {
    let suffix = format!("({})", set_name);
    ws_name.ends_with(&suffix) && ws_name.len() > suffix.len()
}

/// Ice a set: save windows, move to parking
fn ice_set(conn: &mut Connection, state: &State, set_name: &str) -> Result<usize> {
    let tree = conn.get_tree()?;
    let mut snapshots = vec![];

    fn find_workspaces(node: &Node, set_name: &str, snapshots: &mut Vec<WorkspaceSnapshot>) {
        if node.node_type == NodeType::Workspace {
            if let Some(name) = &node.name {
                if workspace_belongs_to_set(name, set_name) {
                    snapshots.extend(snapshot_workspace(node));
                }
            }
        }
        for child in &node.nodes {
            find_workspaces(child, set_name, snapshots);
        }
    }

    find_workspaces(&tree, set_name, &mut snapshots);

    let pinned: Vec<i64> = state.pins()?.iter().map(|pin| pin.con_id).collect();
    strip_pins(&mut snapshots, &pinned);

    if snapshots.is_empty() {
        return Ok(0);
    }

    let window_count: usize = snapshots
        .iter()
        .map(|s| collect_window_ids(&s.tree).len())
        .sum();

    state.save_ice(set_name, &snapshots)?;

    for snapshot in &snapshots {
        for con_id in collect_window_ids(&snapshot.tree) {
            let cmd = format!(
                "[con_id={}] move to workspace \"{}\"",
                con_id, PARKING_WORKSPACE
            );
            let _ = conn.run_command(&cmd);
        }
    }

    Ok(window_count)
}

/// Thaw a set: restore windows from parking
fn thaw_set(conn: &mut Connection, state: &State, set_name: &str) -> Result<(usize, usize)> {
    let snapshots = state.load_ice(set_name)?;

    let total: usize = snapshots
        .iter()
        .map(|s| collect_window_ids(&s.tree).len())
        .sum();

    let mut restored = 0;

    for snapshot in &snapshots {
        restored += restore_tree(conn, &snapshot.tree, &snapshot.name)?;
        let cmd = format!(
            "workspace \"{}\", layout {}",
            snapshot.name, snapshot.layout
        );
        let _ = conn.run_command(&cmd);
    }

    state.remove_ice(set_name)?;

    Ok((restored, total))
}

/// Run sway commands, failing if any of them fails.
fn run(conn: &mut Connection, cmd: &str) -> Result<()> {
    for outcome in conn.run_command(cmd)? {
        outcome.with_context(|| format!("sway command failed: {}", cmd))?;
    }
    Ok(())
}

/// The identifier sway and kanshi match monitors on.
fn monitor_id(make: &str, model: &str, serial: &str) -> String {
    format!("{} {} {}", make, model, serial)
}

#[derive(Debug, Clone)]
struct Monitor {
    id: String,
    /// Connector name, e.g. DP-2; what sway commands take.
    name: String,
    focused: bool,
    visible: Option<String>,
}

/// Connected monitors and the sets they show.
struct Screens {
    monitors: Vec<Monitor>,
    assignments: Assignments,
}

impl Screens {
    fn load(conn: &mut Connection, state: &State) -> Result<Self> {
        let monitors = conn
            .get_outputs()?
            .into_iter()
            .filter(|o| o.active)
            .map(|o| Monitor {
                id: monitor_id(&o.make, &o.model, &o.serial),
                name: o.name,
                focused: o.focused,
                visible: o.current_workspace,
            })
            .collect();
        Ok(Self {
            monitors,
            assignments: state.assignments()?,
        })
    }

    fn ids(&self) -> Vec<String> {
        self.monitors.iter().map(|m| m.id.clone()).collect()
    }

    fn focused(&self) -> Result<&Monitor> {
        self.monitors
            .iter()
            .find(|m| m.focused)
            .context("no focused output")
    }

    fn by_id(&self, id: &str) -> Result<&Monitor> {
        self.monitors
            .iter()
            .find(|m| m.id == id)
            .with_context(|| format!("output {} is not connected", id))
    }

    /// The focused monitor's set, without claiming one for a new monitor.
    fn focused_set(&self, state: &State) -> Result<String> {
        let here = self.focused()?;
        Ok(self
            .assignments
            .resolve(&here.id, &self.ids(), &state.list_sets()?))
    }
}

/// The workspace to show on a monitor taking over `set`: the one it
/// inherits, if that belongs to the set; otherwise where the set was last
/// left; otherwise the set's `A`.
fn visible_or_home(inherited: Option<&str>, remembered: Option<&str>, set: &str) -> String {
    [inherited, remembered]
        .into_iter()
        .flatten()
        .find(|ws| workspace_belongs_to_set(ws, set))
        .map(str::to_string)
        .unwrap_or_else(|| workspace_name('A', set))
}

/// Thaw `set` if it is iced and move its workspaces onto `monitor`. Moves
/// focus; returns whether anything happened.
fn bring(conn: &mut Connection, state: &State, set: &str, monitor: &Monitor) -> Result<bool> {
    let mut changed = false;
    if state.list_iced()?.iter().any(|s| s == set) {
        thaw_set(conn, state, set)?;
        changed = true;
    }
    for ws in conn.get_workspaces()? {
        if workspace_belongs_to_set(&ws.name, set) && ws.output != monitor.name {
            run(
                conn,
                &format!(
                    "workspace \"{}\"; move workspace to output \"{}\"",
                    ws.name, monitor.name
                ),
            )?;
            changed = true;
        }
    }
    Ok(changed)
}

fn show(conn: &mut Connection, monitor: &Monitor, workspace: &str) -> Result<()> {
    run(
        conn,
        &format!(
            "focus output \"{}\"; workspace \"{}\"",
            monitor.name, workspace
        ),
    )
}

/// The focused monitor's set, claiming the default for a monitor seen for
/// the first time. Leaves focus where it was.
fn ensure_set(conn: &mut Connection, state: &State) -> Result<String> {
    let screens = Screens::load(conn, state)?;
    let here = screens.focused()?;
    if let Some(set) = screens.assignments.get(&here.id) {
        return Ok(set.to_string());
    }

    let set = screens.focused_set(state)?;
    let mut assignments = screens.assignments.clone();
    assignments.assign(&here.id, &set);
    state.save_assignments(&assignments)?;
    state.touch_set(&set)?;

    if bring(conn, state, &set, here)? {
        if let Some(ws) = &here.visible {
            run(conn, &format!("workspace \"{}\"", ws))?;
        }
    }
    Ok(set)
}

/// Show `set` on the focused monitor. A set shown on another monitor swaps
/// with this one's; otherwise this monitor's set is iced.
fn change_set(conn: &mut Connection, state: &State, set: &str) -> Result<()> {
    let current = ensure_set(conn, state)?;
    let screens = Screens::load(conn, state)?;
    let here = screens.focused()?;
    let mut assignments = screens.assignments.clone();
    state.touch_set(set)?;

    match screens
        .assignments
        .plan(&here.id, &current, set, &screens.ids())
    {
        Change::Unchanged => {}
        Change::Swap { monitor, old } => {
            let there = screens.by_id(&monitor)?;
            assignments.assign(&here.id, set);
            assignments.assign(&there.id, &old);
            state.save_assignments(&assignments)?;

            state.remember_workspace(&old, here.visible.as_deref())?;
            state.remember_workspace(set, there.visible.as_deref())?;
            let leaving = state.last_workspace(&old)?;
            let arriving = state.last_workspace(set)?;

            bring(conn, state, set, here)?;
            bring(conn, state, &old, there)?;
            show(
                conn,
                there,
                &visible_or_home(here.visible.as_deref(), leaving.as_deref(), &old),
            )?;
            show(
                conn,
                here,
                &visible_or_home(there.visible.as_deref(), arriving.as_deref(), set),
            )?;
        }
        Change::Replace { old } => {
            state.remember_workspace(&old, here.visible.as_deref())?;
            ice_set(conn, state, &old)?;
            assignments.assign(&here.id, set);
            state.save_assignments(&assignments)?;

            let arriving = state.last_workspace(set)?;
            bring(conn, state, set, here)?;
            show(
                conn,
                here,
                &visible_or_home(here.visible.as_deref(), arriving.as_deref(), set),
            )?;
        }
    }
    carry_pins(conn, state, set)
}

/// Keep the sets there is something to go back to: a live workspace, a
/// frozen snapshot, or a monitor showing them. Sway drops a workspace once
/// its last window leaves, so a set with none is gone.
fn retain_live(
    sets: Vec<String>,
    workspaces: &[String],
    iced: &[String],
    shown: &[String],
) -> Vec<String> {
    sets.into_iter()
        .filter(|set| {
            shown.contains(set)
                || iced.contains(set)
                || workspaces.iter().any(|w| workspace_belongs_to_set(w, set))
        })
        .collect()
}

/// The sets to offer, dropping those nothing is left in.
fn prune_sets(conn: &mut Connection, state: &State) -> Result<Vec<String>> {
    let screens = Screens::load(conn, state)?;
    let shown: Vec<String> = screens
        .monitors
        .iter()
        .filter_map(|m| screens.assignments.get(&m.id).map(str::to_string))
        .collect();
    let workspaces: Vec<String> = conn.get_workspaces()?.into_iter().map(|w| w.name).collect();
    let sets = retain_live(state.list_sets()?, &workspaces, &state.list_iced()?, &shown);
    state.save_sets(&sets)?;

    let mut last = state.last_workspaces()?;
    last.retain(|set, _| sets.contains(set));
    state.save_last_workspaces(&last)?;
    Ok(sets)
}

fn wofi_select(prompt: &str, options: &[String]) -> Result<Option<String>> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // Wofi's default sort order lists cached entries first, which would
    // reshuffle the MRU order we feed it. No cache, no reshuffle.
    let mut child = Command::new("wofi")
        .args([
            "--show",
            "dmenu",
            "--cache-file",
            "/dev/null",
            "--prompt",
            prompt,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(options.join("\n").as_bytes())?;
    }

    let output = child.wait_with_output()?;
    let choice = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if choice.is_empty() {
        Ok(None)
    } else {
        Ok(Some(choice))
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let state = State::new()?;
    let mut conn = Connection::new().context("failed to connect to sway")?;

    match cli.command {
        Command::Switch { key } => {
            let set = ensure_set(&mut conn, &state)?;
            run(
                &mut conn,
                &format!("workspace \"{}\"", workspace_name(key, &set)),
            )?;
        }

        Command::Move { key } => {
            let set = ensure_set(&mut conn, &state)?;
            run(
                &mut conn,
                &format!(
                    "move container to workspace \"{}\"",
                    workspace_name(key, &set)
                ),
            )?;
        }

        Command::Set { name } => {
            change_set(&mut conn, &state, &name)?;
        }

        Command::SetMenu => {
            let current = ensure_set(&mut conn, &state)?;
            let sets = prune_sets(&mut conn, &state)?;
            // Drop the set already on this monitor: picking it is a no-op, and
            // leaving it out puts the set left last on top, where wofi's
            // preselection turns the picker into a two-key toggle.
            let sets: Vec<String> = sets.into_iter().filter(|s| *s != current).collect();
            if let Some(choice) = wofi_select("Space set", &sets)? {
                change_set(&mut conn, &state, &choice)?;
            }
        }

        Command::Ice { set } => {
            let set_name = match set {
                Some(set) => set,
                None => Screens::load(&mut conn, &state)?.focused_set(&state)?,
            };
            let window_count = ice_set(&mut conn, &state, &set_name)?;
            if window_count == 0 {
                notify(&format!("Ice: No windows in set '{}'", set_name));
            } else {
                notify(&format!("Iced: {} ({} windows)", set_name, window_count));
            }
        }

        Command::Thaw { set } => {
            let set_name = match set {
                Some(set) => set,
                None => Screens::load(&mut conn, &state)?.focused_set(&state)?,
            };
            match thaw_set(&mut conn, &state, &set_name) {
                Ok((restored, total)) => {
                    notify(&format!(
                        "Thawed: {} ({}/{} windows)",
                        set_name, restored, total
                    ));
                }
                Err(_) => {
                    notify(&format!("No iced set: {}", set_name));
                }
            }
        }

        Command::IceMenu => {
            let iced = state.list_iced()?;
            if iced.is_empty() {
                notify("No iced sets");
                return Ok(());
            }
            if let Some(choice) = wofi_select("Thaw set", &iced)? {
                match thaw_set(&mut conn, &state, &choice) {
                    Ok((restored, total)) => {
                        notify(&format!(
                            "Thawed: {} ({}/{} windows)",
                            choice, restored, total
                        ));
                    }
                    Err(_) => {
                        notify(&format!("Failed to thaw: {}", choice));
                    }
                }
            }
        }

        Command::MoveToSet => {
            let sets = prune_sets(&mut conn, &state)?;
            if let Some(choice) = wofi_select("Move to set", &sets)? {
                state.touch_set(&choice)?;
                state.set_pending_target(&choice)?;
                // Signal sway to enter deliver mode
                conn.run_command("mode deliver")?;
            }
        }

        Command::Deliver { key } => {
            match state.get_pending_target()? {
                Some(target_set) => {
                    let target_ws = workspace_name(key, &target_set);
                    let tree = conn.get_tree()?;
                    let snapshot = find_focused_workspace(&tree).and_then(snapshot_workspace);

                    if let Some(snapshot) = snapshot {
                        let window_ids = collect_window_ids(&snapshot.tree);
                        // A frozen target has no live workspaces to move into:
                        // park the windows and splice them into its snapshot,
                        // so they come back with the set.
                        let frozen = state.list_iced()?.iter().any(|s| *s == target_set);
                        let destination = if frozen {
                            let mut snapshots = state.load_ice(&target_set)?;
                            splice_snapshot(
                                &mut snapshots,
                                &target_ws,
                                &snapshot.layout,
                                snapshot.tree,
                            );
                            state.save_ice(&target_set, &snapshots)?;
                            PARKING_WORKSPACE.to_string()
                        } else {
                            target_ws.clone()
                        };

                        for con_id in &window_ids {
                            let cmd = format!(
                                "[con_id={}] move to workspace \"{}\"",
                                con_id, destination
                            );
                            let _ = conn.run_command(&cmd);
                        }

                        notify(&format!(
                            "Moved {} windows to {}{}",
                            window_ids.len(),
                            target_ws,
                            if frozen { " (iced)" } else { "" }
                        ));
                    }

                    state.clear_pending_target()?;
                    conn.run_command("mode default")?;
                }
                None => {
                    notify("No pending move target");
                    conn.run_command("mode default")?;
                }
            }
        }

        Command::Pin { key } => {
            let set = ensure_set(&mut conn, &state)?;
            let tree = conn.get_tree()?;
            let con_id = focused_window_id(&tree).context("no focused window to pin")?;
            let mut pins = state.pins()?;

            // Bare `pin` toggles: with no key to move the window to, a
            // second press on a pinned window can only mean release it.
            if key.is_none() && pins.iter().any(|pin| pin.con_id == con_id) {
                pins.retain(|pin| pin.con_id != con_id);
                state.save_pins(&pins)?;
                notify("Unpinned");
                return Ok(());
            }

            let key = match key {
                Some(key) => key,
                None => find_focused_workspace(&tree)
                    .and_then(|ws| ws.name.as_deref())
                    .and_then(|name| workspace_key(name, &set))
                    .context("focused workspace has no single-character key")?,
            };
            pins.retain(|pin| pin.con_id != con_id);
            pins.push(Pin { con_id, key });
            state.save_pins(&pins)?;
            carry_pins(&mut conn, &state, &set)?;
            notify(&format!("Pinned to {}", key.to_ascii_uppercase()));
        }

        Command::Unpin => {
            let con_id =
                focused_window_id(&conn.get_tree()?).context("no focused window to unpin")?;
            let mut pins = state.pins()?;
            pins.retain(|pin| pin.con_id != con_id);
            state.save_pins(&pins)?;
        }

        Command::Current => {
            println!("{}", Screens::load(&mut conn, &state)?.focused_set(&state)?);
        }

        Command::ListIced => {
            for set in state.list_iced()? {
                println!("{}", set);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::assignments::Assignments;
    use crate::{
        extract_tree, monitor_id, retain_live, strip_pins, visible_or_home,
        workspace_belongs_to_set, workspace_key, workspace_name, State, TreeNode,
        WorkspaceSnapshot,
    };
    use serde_json::{json, Value};
    use std::fs;
    use std::path::PathBuf;
    use swayipc::Node;

    /// A fresh state directory, removed on drop.
    struct TempState {
        dir: PathBuf,
        state: State,
    }

    impl TempState {
        fn new(name: &str) -> anyhow::Result<Self> {
            let dir =
                std::env::temp_dir().join(format!("space-test-{}-{}", std::process::id(), name));
            let _ = fs::remove_dir_all(&dir);
            let state = State::at(dir.clone())?;
            Ok(Self { dir, state })
        }
    }

    impl Drop for TempState {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn state_without_assignments_file_is_empty() -> anyhow::Result<()> {
        let t = TempState::new("empty")?;
        assert_eq!(t.state.assignments()?, Assignments::default());
        Ok(())
    }

    #[test]
    fn state_round_trips_assignments() -> anyhow::Result<()> {
        let t = TempState::new("round-trip")?;
        let mut a = Assignments::default();
        a.assign("Dell Inc. DELL U3415W PXF798BL0E4L", "work");
        t.state.save_assignments(&a)?;
        assert_eq!(t.state.assignments()?, a);
        Ok(())
    }

    #[test]
    fn state_rejects_corrupt_assignments_file() -> anyhow::Result<()> {
        let t = TempState::new("corrupt")?;
        fs::write(t.dir.join("monitor_sets.json"), "not json")?;
        let err = match t.state.assignments() {
            Ok(a) => panic!("parsed corrupt file as {:?}", a),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("monitor_sets.json"), "{}", err);
        Ok(())
    }

    #[test]
    fn touch_set_moves_to_front_once() -> anyhow::Result<()> {
        let t = TempState::new("touch-set")?;
        t.state.touch_set("adhoc")?;
        t.state.touch_set("adhoc")?;
        t.state.touch_set("work")?;
        assert_eq!(t.state.list_sets()?, ["work", "adhoc", "main", "scratch"]);
        Ok(())
    }

    #[test]
    fn retain_live_keeps_only_sets_with_something_left() {
        let sets = ["main", "work", "scratch", "dead"].map(str::to_string).to_vec();
        let live = retain_live(
            sets,
            &["A(main)".to_string(), "_".to_string()],
            &["work".to_string()],
            &["scratch".to_string()],
        );
        assert_eq!(live, ["main", "work", "scratch"]);
    }

    #[test]
    fn monitor_id_matches_sway_identifier() {
        assert_eq!(
            monitor_id("Sharp Corporation", "LQ156M1JW03", "Unknown"),
            "Sharp Corporation LQ156M1JW03 Unknown"
        );
    }

    #[test]
    fn workspace_name_uppercases_key() {
        assert_eq!(workspace_name('q', "work"), "Q(work)");
    }

    #[test]
    fn workspace_membership_reads_suffix() {
        assert!(workspace_belongs_to_set("Q(work)", "work"));
        assert!(workspace_belongs_to_set("foo(bar)(main)", "main"));
        assert!(!workspace_belongs_to_set("Q(work)", "main"));
        assert!(!workspace_belongs_to_set("(work)", "work"));
        assert!(!workspace_belongs_to_set("Q(homework)", "work"));
        assert!(!workspace_belongs_to_set("_", "work"));
    }

    #[test]
    fn workspace_key_reads_single_character_prefix() {
        assert_eq!(workspace_key("Z(work)", "work"), Some('Z'));
        assert_eq!(workspace_key("foo(bar)(work)", "work"), None);
        assert_eq!(workspace_key("Z(work)", "main"), None);
        assert_eq!(workspace_key("_", "work"), None);
    }

    #[test]
    fn visible_or_home_keeps_inherited_workspace_of_set() {
        assert_eq!(
            visible_or_home(Some("S(work)"), Some("Q(work)"), "work"),
            "S(work)"
        );
    }

    #[test]
    fn visible_or_home_returns_to_remembered_workspace() {
        assert_eq!(
            visible_or_home(Some("S(main)"), Some("Q(work)"), "work"),
            "Q(work)"
        );
    }

    #[test]
    fn visible_or_home_falls_back_to_a() {
        assert_eq!(visible_or_home(Some("S(main)"), None, "work"), "A(work)");
        assert_eq!(visible_or_home(None, None, "work"), "A(work)");
        assert_eq!(
            visible_or_home(Some("_"), Some("Q(main)"), "work"),
            "A(work)"
        );
    }

    #[test]
    fn remember_workspace_ignores_foreign_workspace() -> anyhow::Result<()> {
        let t = TempState::new("remember")?;
        t.state.remember_workspace("work", Some("Q(work)"))?;
        t.state.remember_workspace("work", Some("S(main)"))?;
        t.state.remember_workspace("work", None)?;
        assert_eq!(t.state.last_workspace("work")?.as_deref(), Some("Q(work)"));
        Ok(())
    }

    /// A node as sway's get_tree reports it.
    fn node(id: i64, kind: &str, pid: Option<i32>, layout: &str, nodes: Vec<Value>) -> Value {
        let rect = json!({"x": 0, "y": 0, "width": 0, "height": 0});
        json!({
            "id": id, "name": format!("n{}", id), "type": kind, "border": "normal",
            "current_border_width": 0, "layout": layout, "percent": null,
            "rect": rect, "window_rect": rect, "deco_rect": rect, "geometry": rect,
            "urgent": false, "focused": false, "focus": [], "nodes": nodes,
            "floating_nodes": [], "sticky": false, "pid": pid,
            "app_id": pid.map(|_| "foot"),
        })
    }

    fn window(id: i64) -> Value {
        node(id, "con", Some(1), "none", vec![])
    }

    fn tree(value: Value) -> Option<TreeNode> {
        let node: Node = serde_json::from_value(value).expect("valid sway node");
        extract_tree(&node, false)
    }

    fn window_ids(node: &TreeNode) -> Vec<i64> {
        match node {
            TreeNode::Window { con_id, .. } => vec![*con_id],
            TreeNode::Container { children, .. } => children.iter().flat_map(window_ids).collect(),
        }
    }

    #[test]
    fn extract_tree_reads_window() {
        match tree(window(7)) {
            Some(TreeNode::Window {
                con_id,
                app_id,
                floating,
                ..
            }) => {
                assert_eq!(con_id, 7);
                assert_eq!(app_id.as_deref(), Some("foot"));
                assert!(!floating);
            }
            other => panic!("expected window, got {:?}", other),
        }
    }

    #[test]
    fn extract_tree_drops_empty_container() {
        assert!(tree(node(1, "con", None, "splith", vec![])).is_none());
    }

    #[test]
    fn extract_tree_unwraps_single_window_container() {
        match tree(node(1, "con", None, "splitv", vec![window(7)])) {
            Some(TreeNode::Window { con_id, .. }) => assert_eq!(con_id, 7),
            other => panic!("expected bare window, got {:?}", other),
        }
    }

    #[test]
    fn extract_tree_keeps_nested_layout() {
        let inner = node(2, "con", None, "tabbed", vec![window(8), window(9)]);
        let outer = node(1, "con", None, "splitv", vec![window(7), inner]);
        match tree(outer) {
            Some(TreeNode::Container { layout, children }) => {
                assert_eq!(layout, "splitv");
                assert_eq!(children.len(), 2);
                match &children[1] {
                    TreeNode::Container { layout, .. } => assert_eq!(layout, "tabbed"),
                    other => panic!("expected tabbed container, got {:?}", other),
                }
                assert_eq!(
                    children.iter().flat_map(window_ids).collect::<Vec<_>>(),
                    [7, 8, 9]
                );
            }
            other => panic!("expected container, got {:?}", other),
        }
    }

    #[test]
    fn strip_pins_empties_containers_and_workspaces() {
        let snapshot = |name: &str, tree: Vec<TreeNode>| WorkspaceSnapshot {
            name: name.to_string(),
            layout: "splith".to_string(),
            tree,
        };
        let win = |id| TreeNode::Window {
            con_id: id,
            app_id: None,
            name: None,
            floating: false,
        };
        let mut snapshots = vec![
            snapshot(
                "Q(work)",
                vec![
                    win(1),
                    TreeNode::Container {
                        layout: "splitv".to_string(),
                        children: vec![win(2)],
                    },
                ],
            ),
            snapshot("Z(work)", vec![win(3)]),
        ];

        strip_pins(&mut snapshots, &[2, 3]);

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].name, "Q(work)");
        assert_eq!(
            snapshots[0].tree.iter().flat_map(window_ids).collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn extract_tree_ignores_workspaces() {
        assert!(tree(node(1, "workspace", None, "splith", vec![window(7)])).is_none());
    }
}
