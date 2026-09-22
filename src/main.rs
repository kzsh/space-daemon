use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

use std::fs;
use std::path::PathBuf;
use swayipc::{Connection, Node, NodeLayout, NodeType};

const KEYS: [char; 15] = [
    'Q', 'W', 'E', 'R', 'T', 'A', 'S', 'D', 'F', 'G', 'Z', 'X', 'C', 'V', 'B',
];
const PARKING_WORKSPACE: &str = "_";

#[derive(Parser)]
#[command(name = "space", about = "Sway workspace set manager")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Switch to workspace <key> in current set
    Switch { key: char },
    /// Move focused window to workspace <key> in current set
    Move { key: char },
    /// Change current set (auto ice/thaw)
    Set { name: String },
    /// Pick set via wofi (auto ice/thaw)
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
    /// Print current set
    Current,
    /// List frozen sets
    ListIced,
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
        fs::create_dir_all(&dir)?;
        fs::create_dir_all(dir.join("ice"))?;
        Ok(Self { dir })
    }

    fn current_set(&self) -> Result<String> {
        let path = self.dir.join("current_space_set");
        match fs::read_to_string(&path) {
            Ok(s) => Ok(s.trim().to_string()),
            Err(_) => Ok("main".to_string()),
        }
    }

    fn set_current(&self, name: &str) -> Result<()> {
        fs::write(self.dir.join("current_space_set"), name)?;
        Ok(())
    }

    fn sets_file(&self) -> PathBuf {
        self.dir.join("space_sets")
    }

    fn list_sets(&self) -> Result<Vec<String>> {
        match fs::read_to_string(self.sets_file()) {
            Ok(s) => Ok(s.lines().map(|l| l.to_string()).collect()),
            Err(_) => Ok(vec!["main".into(), "work".into(), "scratch".into()]),
        }
    }

    fn add_set(&self, name: &str) -> Result<()> {
        let mut sets = self.list_sets()?;
        if !sets.contains(&name.to_string()) {
            sets.push(name.to_string());
            fs::write(self.sets_file(), sets.join("\n") + "\n")?;
        }
        Ok(())
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
            } else if children.len() == 1 && matches!(&children[0], TreeNode::Window { .. }) {
                // Single window, don't wrap in container
                Some(children.into_iter().next().unwrap())
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
    if node.node_type == NodeType::Workspace {
        if has_focused_descendant(node) {
            return Some(node);
        }
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

/// Collect all window IDs from a workspace node
fn collect_workspace_windows(node: &Node, ids: &mut Vec<i64>) {
    if node.node_type == NodeType::Con && node.pid.is_some() {
        ids.push(node.id);
    }
    for child in &node.nodes {
        collect_workspace_windows(child, ids);
    }
    for child in &node.floating_nodes {
        collect_workspace_windows(child, ids);
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

fn restore_node(conn: &mut Connection, node: &TreeNode, workspace: &str, is_first: bool) -> Result<usize> {
    match node {
        TreeNode::Window { con_id, floating, .. } => {
            let cmd = format!("[con_id={}] move to workspace \"{}\"", con_id, workspace);
            if conn.run_command(&cmd).is_ok() {
                if *floating {
                    let _ = conn.run_command(&format!("[con_id={}] floating enable", con_id));
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

fn notify(msg: &str) {
    let _ = std::process::Command::new("notify-send")
        .arg(msg)
        .spawn();
}

/// Ice a set: save windows, move to parking
fn ice_set(conn: &mut Connection, state: &State, set_name: &str) -> Result<usize> {
    let tree = conn.get_tree()?;
    
    let target_workspaces: Vec<String> = KEYS
        .iter()
        .map(|k| workspace_name(*k, set_name))
        .collect();
    
    let mut snapshots = vec![];
    
    fn find_workspaces(node: &Node, targets: &[String], snapshots: &mut Vec<WorkspaceSnapshot>) {
        if node.node_type == NodeType::Workspace {
            if let Some(name) = &node.name {
                if targets.contains(name) {
                    let mut tree_nodes: Vec<TreeNode> = node
                        .nodes
                        .iter()
                        .filter_map(|n| extract_tree(n, false))
                        .collect();
                    
                    for floating in &node.floating_nodes {
                        if let Some(tn) = extract_tree(floating, true) {
                            tree_nodes.push(tn);
                        }
                    }
                    
                    if !tree_nodes.is_empty() {
                        snapshots.push(WorkspaceSnapshot {
                            name: name.clone(),
                            layout: layout_to_string(node.layout),
                            tree: tree_nodes,
                        });
                    }
                }
            }
        }
        for child in &node.nodes {
            find_workspaces(child, targets, snapshots);
        }
    }
    
    find_workspaces(&tree, &target_workspaces, &mut snapshots);
    
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
            let cmd = format!("[con_id={}] move to workspace \"{}\"", con_id, PARKING_WORKSPACE);
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
        let cmd = format!("workspace \"{}\", layout {}", snapshot.name, snapshot.layout);
        let _ = conn.run_command(&cmd);
    }
    
    state.remove_ice(set_name)?;
    
    Ok((restored, total))
}

fn wofi_select(prompt: &str, options: &[String]) -> Result<Option<String>> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    
    let mut child = Command::new("wofi")
        .args(["--show", "dmenu", "--prompt", prompt])
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
            let set = state.current_set()?;
            let ws = workspace_name(key, &set);
            conn.run_command(format!("workspace \"{}\"", ws))?;
        }

        Command::Move { key } => {
            let set = state.current_set()?;
            let ws = workspace_name(key, &set);
            conn.run_command(format!("move container to workspace \"{}\"", ws))?;
        }

        Command::Set { name } => {
            let old_set = state.current_set()?;
            if old_set != name {
                // Ice old set
                ice_set(&mut conn, &state, &old_set)?;
                
                // Switch
                state.add_set(&name)?;
                state.set_current(&name)?;
                
                // Thaw new set if it was iced
                if state.list_iced()?.contains(&name) {
                    thaw_set(&mut conn, &state, &name)?;
                }
                
                // Switch to A workspace in new set
                let ws = workspace_name('A', &name);
                conn.run_command(format!("workspace \"{}\"", ws))?;
            }
        }

        Command::SetMenu => {
            let sets = state.list_sets()?;
            if let Some(choice) = wofi_select("Space set", &sets)? {
                let old_set = state.current_set()?;
                if old_set != choice {
                    // Ice old set
                    ice_set(&mut conn, &state, &old_set)?;
                    
                    // Switch
                    state.add_set(&choice)?;
                    state.set_current(&choice)?;
                    
                    // Thaw new set if it was iced
                    if state.list_iced()?.contains(&choice) {
                        thaw_set(&mut conn, &state, &choice)?;
                    }
                    
                    // Switch to A workspace in new set
                    let ws = workspace_name('A', &choice);
                    conn.run_command(format!("workspace \"{}\"", ws))?;
                }
            }
        }

        Command::Ice { set } => {
            let set_name = set.unwrap_or_else(|| state.current_set().unwrap_or_default());
            let window_count = ice_set(&mut conn, &state, &set_name)?;
            if window_count == 0 {
                notify(&format!("Ice: No windows in set '{}'", set_name));
            } else {
                notify(&format!("Iced: {} ({} windows)", set_name, window_count));
            }
        }

        Command::Thaw { set } => {
            let set_name = set.unwrap_or_else(|| state.current_set().unwrap_or_default());
            match thaw_set(&mut conn, &state, &set_name) {
                Ok((restored, total)) => {
                    notify(&format!("Thawed: {} ({}/{} windows)", set_name, restored, total));
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
                        notify(&format!("Thawed: {} ({}/{} windows)", choice, restored, total));
                    }
                    Err(_) => {
                        notify(&format!("Failed to thaw: {}", choice));
                    }
                }
            }
        }

        Command::MoveToSet => {
            let sets = state.list_sets()?;
            if let Some(choice) = wofi_select("Move to set", &sets)? {
                state.add_set(&choice)?;
                state.set_pending_target(&choice)?;
                // Signal sway to enter deliver mode
                conn.run_command("mode deliver")?;
            }
        }

        Command::Deliver { key } => {
            match state.get_pending_target()? {
                Some(target_set) => {
                    let target_ws = workspace_name(key, &target_set);
                    
                    // Get current workspace and all its windows
                    let tree = conn.get_tree()?;
                    let focused = find_focused_workspace(&tree);
                    
                    if let Some(current_ws) = focused {
                        // Collect all window IDs from current workspace
                        let mut window_ids = vec![];
                        collect_workspace_windows(&current_ws, &mut window_ids);
                        
                        // Move all windows to target
                        for con_id in &window_ids {
                            let cmd = format!("[con_id={}] move to workspace \"{}\"", con_id, target_ws);
                            let _ = conn.run_command(&cmd);
                        }
                        
                        notify(&format!("Moved {} windows to {}", window_ids.len(), target_ws));
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

        Command::Current => {
            println!("{}", state.current_set()?);
        }

        Command::ListIced => {
            for set in state.list_iced()? {
                println!("{}", set);
            }
        }
    }

    Ok(())
}
