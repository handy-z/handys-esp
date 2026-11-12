//! The text the dashboard rows and the `--status` file show.
use std::cmp::Ordering;
use std::collections::BTreeMap;

use handy_sdk::ui::dashboard::TreeItem;

use crate::esp::engine::EngineView;
use crate::esp::marker::Marker;
use crate::esp::targets::{Path, Targets, has_path, join_list, join_paths, show_path};
use crate::game::scanner::TreeNode;

const DOT: &str = " \u{B7} ";

fn lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

// Explorer order: by name ignoring case, exact spelling breaking ties (Roblox names are case-sensitive).
#[derive(PartialEq, Eq)]
struct ByName(String);

impl Ord for ByName {
    fn cmp(&self, other: &Self) -> Ordering {
        let (la, lb) = (lower(&self.0), lower(&other.0));
        if la != lb { la.cmp(&lb) } else { self.0.cmp(&other.0) }
    }
}

impl PartialOrd for ByName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn detail_of(n: &TreeNode) -> String {
    let count = if n.count > 1 {
        format!("\u{D7}{}{DOT}", n.count)
    } else {
        String::new()
    };
    let s = count + &n.cls;
    match n.nearest {
        Some(d) => s + DOT + &distance_text(d),
        None => s,
    }
}

/// One Pick tree item: the path it stands for and what the tree shows ("Coin · ×40 · Model · 12 studs").
#[derive(Clone, Debug, PartialEq)]
pub struct PickNode {
    /// Empty for Workspace, which has no box.
    pub path: Path,
    pub item: TreeItem,
}

struct N {
    name: String,
    detail: String,
    kids: BTreeMap<ByName, usize>,
}

impl N {
    fn new(name: &str, detail: &str) -> Self {
        Self {
            name: name.to_owned(),
            detail: detail.to_owned(),
            kids: BTreeMap::new(),
        }
    }
}

// hit: the name holds q; under: something below does.
fn mark(ns: &[N], q: &str, hit: &mut [bool], under: &mut [bool], n: usize) -> bool {
    for &kid in ns[n].kids.values() {
        if mark(ns, q, hit, under, kid) {
            under[n] = true;
        }
    }
    hit[n] = n != 0 && lower(&ns[n].name).contains(q);
    hit[n] || under[n]
}

struct Visit<'a> {
    ns: &'a [N],
    t: &'a Targets,
    q: &'a str,
    hit: &'a [bool],
    under: &'a [bool],
    out: Vec<PickNode>,
}

impl Visit<'_> {
    fn leads_to_pick(&self, p: &[String]) -> bool {
        self.t.paths.iter().any(|q| q.len() > p.len() && q.starts_with(p))
    }

    fn visit(&mut self, n: usize, parent: Option<usize>, path: &Path, in_hit: bool) {
        if !in_hit && !self.hit[n] && !self.under[n] && n != 0 {
            return;
        }
        let me = self.out.len();
        let node = &self.ns[n];
        let item = TreeItem {
            key: if n != 0 {
                join_paths(std::slice::from_ref(path))
            } else {
                String::new()
            },
            parent,
            text: if node.detail.is_empty() {
                node.name.clone()
            } else {
                format!("{}{DOT}{}", node.name, node.detail)
            },
            checked: has_path(self.t, path),
            checkbox: n != 0,
            open: n == 0 || self.leads_to_pick(path) || (!self.q.is_empty() && self.under[n]),
        };
        self.out.push(PickNode {
            path: path.clone(),
            item,
        });
        for (name, &kid) in &node.kids {
            let mut next = path.clone();
            next.push(name.0.clone());
            self.visit(kid, Some(me), &next, in_hit || self.hit[n]);
        }
    }
}

/// The tree to pick from, in pre-order with each level sorted by name: the scan's places plus picked paths not in the
/// game right now ("none here"). Workspace and the folders leading to a pick start open. A non-empty `find` keeps only
/// names holding it (any case), what's under them and the folders leading to them, which open.
pub fn pick_tree(tree: &[TreeNode], t: &Targets, find: &str) -> Vec<PickNode> {
    let mut ns = vec![N::new("Workspace", "")];
    let mut at = vec![0usize; tree.len()]; // scan index -> ns index; tree[0] is Workspace
    for (i, n) in tree.iter().enumerate().skip(1) {
        at[i] = ns.len();
        ns.push(N::new(&n.name, &detail_of(n)));
        ns[at[n.parent as usize]]
            .kids
            .entry(ByName(n.name.clone()))
            .or_insert(at[i]);
    }
    for p in &t.paths {
        // picked but not in the game right now: still there to untick
        let mut cur = 0;
        for name in p {
            if let Some(&kid) = ns[cur].kids.get(&ByName(name.clone())) {
                cur = kid;
                continue;
            }
            ns.push(N::new(name, "none here"));
            let added = ns.len() - 1;
            ns[cur].kids.entry(ByName(name.clone())).or_insert(added);
            cur = added;
        }
    }
    let q = lower(find);
    let (mut hit, mut under) = (vec![false; ns.len()], vec![false; ns.len()]);
    mark(&ns, &q, &mut hit, &mut under, 0);
    let mut v = Visit {
        ns: &ns,
        t,
        q: &q,
        hit: &hit,
        under: &under,
        out: Vec::new(),
    };
    v.visit(0, None, &Path::new(), false);
    v.out
}

/// "Game 65241", or "-" before the first scan.
pub fn game_text(game: i64) -> String {
    if game != 0 { format!("Game {game}") } else { "-".into() }
}

pub fn targets_text(t: &Targets) -> String {
    if t.is_empty() {
        return "Nothing yet: tick things on the Pick page".into();
    }
    let mut s = String::new();
    for p in &t.paths {
        s += if s.is_empty() { "" } else { ", " };
        s += &show_path(p);
    }
    if !t.names.is_empty() {
        s += if s.is_empty() { "" } else { "\n" };
        s += &format!("Names: {}", join_list(&t.names));
    }
    if !t.classes.is_empty() {
        s += if s.is_empty() { "" } else { "\n" };
        s += &format!("Classes: {}", join_list(&t.classes));
    }
    s
}

pub fn found_text(v: &EngineView) -> String {
    if !v.on {
        return "ESP is off".into();
    }
    let Some(frame) = &v.frame else { return "-".into() };
    let mut s = format!(
        "{} found{DOT}{} on screen",
        v.scanned.as_ref().map_or(0, |s| s.scan.matches.len()),
        frame.markers.len()
    );
    if v.frame_ms > 0.0 {
        // only while drawing
        s += &format!("{DOT}{:.1} ms a frame", v.frame_ms);
    }
    s
}

/// "45 studs"
pub fn distance_text(studs: f32) -> String {
    format!("{} studs", studs.round() as i64)
}

/// "Chest · 45 studs · (100, 200)-(150, 260)"
pub fn marker_line(m: &Marker) -> String {
    let px = |f: f32| (f.round() as i64).to_string();
    let mut s = m.name.clone();
    if let Some(studs) = m.studs {
        s += DOT;
        s += &distance_text(studs);
    }
    let b = &m.r#box;
    format!(
        "{s}{DOT}({}, {})-({}, {})",
        px(b.left),
        px(b.top),
        px(b.right),
        px(b.bottom)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::esp::marker::Frame;
    use handy_sdk::roblox::screen::Vec2;

    fn node(name: &str, cls: &str, parent: i32, count: i32, nearest: Option<f32>) -> TreeNode {
        TreeNode {
            name: name.into(),
            cls: cls.into(),
            parent,
            count,
            nearest,
        }
    }

    // Workspace { Loot (Folder) { coin ×2, Chest (12 studs) }, Map (Model) }
    fn tree() -> Vec<TreeNode> {
        vec![
            node("Workspace", "Workspace", -1, 1, None),
            node("Loot", "Folder", 0, 1, None),
            node("coin", "Part", 1, 2, None),
            node("Chest", "Model", 1, 1, Some(12.4)),
            node("Map", "Model", 0, 1, None),
        ]
    }

    fn p(s: &[&str]) -> Path {
        s.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn view_pick_tree_sorted_with_picks_and_absent_paths() {
        let t = Targets {
            paths: vec![p(&["Loot", "Chest"]), p(&["Gone", "Gem"])],
            ..Default::default()
        };
        let nodes = pick_tree(&tree(), &t, "");
        assert_eq!(nodes.len(), 7);
        // pre-order, each level by name ignoring case: Workspace, Gone, Gem, Loot, Chest, coin, Map
        let i = |k: usize| &nodes[k].item;
        assert!(nodes[0].path.is_empty() && !i(0).checkbox && i(0).open && i(0).parent.is_none());
        assert!(nodes[1].path == p(&["Gone"]) && i(1).text == "Gone \u{B7} none here" && i(1).open);
        assert!(nodes[2].path == p(&["Gone", "Gem"]) && i(2).checked && i(2).parent == Some(1));
        assert!(nodes[3].path == p(&["Loot"]) && i(3).text == "Loot \u{B7} Folder" && i(3).open);
        assert!(nodes[4].path == p(&["Loot", "Chest"]) && i(4).checked && !i(4).open);
        assert_eq!(i(4).text, "Chest \u{B7} Model \u{B7} 12 studs");
        assert!(i(5).text == "coin \u{B7} \u{D7}2 \u{B7} Part" && !i(5).checked && i(5).parent == Some(3));
        assert!(nodes[6].path == p(&["Map"]) && !i(6).open && i(6).parent == Some(0));
        assert!(i(4).key != i(5).key && i(0).key.is_empty());
    }

    #[test]
    fn view_pick_tree_find_keeps_matches_what_leads_there_and_what_is_under() {
        let names = |ns: Vec<PickNode>| -> Vec<String> {
            ns.iter()
                .map(|n| {
                    let first = n.item.text.split(' ').next().unwrap_or_default();
                    format!("{first}{}", if n.item.open { "+" } else { "" })
                })
                .collect()
        };
        let none = Targets::default();
        // "CO" holds coin in any case: Loot leads there and opens
        assert_eq!(names(pick_tree(&tree(), &none, "CO")), ["Workspace+", "Loot+", "coin"]);
        // a matched folder keeps all it holds, closed
        assert_eq!(
            names(pick_tree(&tree(), &none, "loot")),
            ["Workspace+", "Loot", "Chest", "coin"]
        );
        assert_eq!(names(pick_tree(&tree(), &none, "zzz")), ["Workspace+"]);
        assert_eq!(pick_tree(&tree(), &none, "").len(), tree().len());
    }

    #[test]
    fn view_game_and_targets_text() {
        assert_eq!(game_text(65241), "Game 65241");
        assert_eq!(game_text(0), "-");
        let t = Targets {
            classes: p(&["Tool"]),
            paths: vec![p(&["Loot", "Chest"])],
            ..Default::default()
        };
        assert_eq!(targets_text(&t), "Loot \u{203A} Chest\nClasses: Tool");
    }

    #[test]
    fn view_found_text() {
        let mut v = EngineView {
            on: false,
            ..Default::default()
        };
        assert_eq!(found_text(&v), "ESP is off");
        v.on = true;
        assert_eq!(found_text(&v), "-");
        v.frame = Some(Frame {
            size: Vec2 { x: 800., y: 600. },
            markers: vec![Marker::default(), Marker::default()],
        });
        assert_eq!(found_text(&v), "0 found \u{B7} 2 on screen");
        v.frame_ms = 1.42;
        assert_eq!(found_text(&v), "0 found \u{B7} 2 on screen \u{B7} 1.4 ms a frame");
    }
}
