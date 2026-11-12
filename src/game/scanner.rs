use std::collections::{BTreeMap, HashMap, HashSet};

use handy_sdk::roblox::instance::{child_by_class, children, data_model, instance_class, instance_name};
use handy_sdk::roblox::memory::Memory;
use handy_sdk::roblox::offsets::Offsets;
use handy_sdk::roblox::process::RobloxProcess;
use handy_sdk::roblox::read::{
    character_of, is_base_part, local_root_position, part_position, primary_part, primitive_of, workspace_of,
};
use handy_sdk::roblox::screen::{Vec3, distance, read_screen_view};

use crate::esp::marker::{Frame, Part, marker_for};
use crate::esp::targets::{Path, Targets, matches};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Match {
    pub inst: usize,
    pub name: String,
    /// BasePart.Primitive of the match itself or its parts.
    pub primitives: Vec<usize>,
}

/// One place in Workspace's tree: every instance with the same names from Workspace down is one node, so 40 Coins in
/// a folder are one "Coin" with count 40 (and their children merge the same way).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeNode {
    pub name: String,
    /// cls of the first one seen
    pub cls: String,
    /// Index in Scan::tree; -1 for Workspace at index 0.
    pub parent: i32,
    pub count: i32,
    /// The nearest one that has a position, from you.
    pub nearest: Option<f32>,
}

/// Bounds one walk of Workspace (each visited instance costs about six reads).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScanLimits {
    pub depth: i32,
    pub visits: usize,
    pub matches: usize,
    pub parts_per_match: usize,
    pub nodes: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            depth: 12,
            visits: 20000,
            matches: 128,
            parts_per_match: 16,
            nodes: 5000,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scan {
    /// Tree order; nothing inside a match is matched again.
    pub matches: Vec<Match>,
    /// Workspace first, every parent before its children; only places that are, or hold, something drawable (a part,
    /// or a Model/Tool/Folder/Accessory).
    pub tree: Vec<TreeNode>,
    /// DataModel.GameId (scan_roblox only)
    pub game: i64,
    /// The Players service (scan_roblox only)
    pub players: usize,
}

// No parts of their own, but the ESP boxes the parts inside. Welds, sounds, labels and the like hold none.
fn is_container(cls: &str) -> bool {
    matches!(cls, "Model" | "Tool" | "Folder" | "Accessory")
}

// The picked paths as a tree of names, so each instance finds its place from its parent's in one lookup.
#[derive(Default)]
struct PathTrie {
    kids: BTreeMap<String, PathTrie>,
    picked: bool,
}

fn trie_of(paths: &[Path]) -> PathTrie {
    let mut root = PathTrie::default();
    for p in paths {
        let mut n = &mut root;
        for name in p {
            n = n.kids.entry(name.clone()).or_default();
        }
        n.picked = true;
    }
    root
}

// A part's position, or a model's PrimaryPart's.
fn position_of(m: &dyn Memory, o: &Offsets, inst: usize, cls: &str) -> Option<Vec3> {
    let part = if is_base_part(cls) {
        inst
    } else if cls == "Model" {
        primary_part(m, o, inst)
    } else {
        0
    };
    part_position(m, o, part)
}

// The match's own primitive if it is a part, else those of its descendant parts (breadth first).
fn primitives_of(m: &dyn Memory, o: &Offsets, inst: usize, cls: &str, limit: usize) -> Vec<usize> {
    let mut out = Vec::new();
    if is_base_part(cls) {
        let p = primitive_of(m, o, inst);
        if p != 0 {
            out.push(p);
        }
        return out;
    }
    let mut queue = vec![(inst, 0)];
    let mut i = 0;
    while i < queue.len() && out.len() < limit && queue.len() < 512 {
        let (node, depth) = queue[i];
        for c in children(m, o, node) {
            if out.len() >= limit {
                break;
            }
            if is_base_part(&instance_class(m, o, c).unwrap_or_default()) {
                let p = primitive_of(m, o, c);
                if p != 0 {
                    out.push(p);
                }
            } else if depth < 3 {
                queue.push((c, depth + 1));
            }
        }
        i += 1;
    }
    out
}

#[derive(Clone, Copy)]
struct Entry<'a> {
    inst: usize,
    depth: i32,
    /// -1 past the node limit
    parent_node: i32,
    /// Where the parent sits among the picks; None when off every picked path.
    parent_pick: Option<&'a PathTrie>,
    in_match: bool,
    in_picked_folder: bool,
}

/// One walk of data_model's Workspace that both matches the targets and builds the tree to pick from. Players'
/// characters, your own included, are never walked into.
pub fn scan_workspace(
    m: &dyn Memory,
    o: &Offsets,
    dm: usize,
    t: &Targets,
    from: Option<Vec3>,
    lim: &ScanLimits,
) -> Scan {
    let mut out = Scan::default();
    let ws = workspace_of(m, o, dm);
    if ws == 0 {
        return out;
    }
    let mut characters = HashSet::new();
    for pl in children(m, o, child_by_class(m, o, dm, "Players")) {
        let c = character_of(m, o, pl);
        if c != 0 {
            characters.insert(c);
        }
    }

    let picks = trie_of(&t.paths);
    // Every visited instance gets a node first; at the end only nodes that are or hold something drawable stay.
    let mut nodes = vec![TreeNode {
        name: "Workspace".into(),
        cls: "Workspace".into(),
        parent: -1,
        count: 1,
        nearest: None,
    }];
    let mut kids: Vec<HashMap<String, i32>> = vec![HashMap::new()]; // per node: child node by name
    let mut keep = vec![true];

    let mut stack: Vec<Entry> = Vec::new();
    fn push<'a>(m: &dyn Memory, o: &Offsets, stack: &mut Vec<Entry<'a>>, parent: usize, e: Entry<'a>) {
        // reversed, so it pops in tree order
        for c in children(m, o, parent).into_iter().rev() {
            stack.push(Entry { inst: c, ..e });
        }
    }
    push(
        m,
        o,
        &mut stack,
        ws,
        Entry {
            inst: 0,
            depth: 1,
            parent_node: 0,
            parent_pick: Some(&picks),
            in_match: false,
            in_picked_folder: false,
        },
    );
    let mut visits = 0;
    while visits < lim.visits {
        let Some(e) = stack.pop() else { break };
        visits += 1;
        if characters.contains(&e.inst) {
            continue;
        }
        let (Some(name), Some(cls)) = (instance_name(m, o, e.inst), instance_class(m, o, e.inst)) else {
            continue;
        };

        let mut node = -1;
        if e.parent_node >= 0 {
            let siblings = &mut kids[e.parent_node as usize];
            if let Some(&n) = siblings.get(&name) {
                node = n;
            } else if nodes.len() < lim.nodes {
                node = nodes.len() as i32;
                siblings.insert(name.clone(), node);
                nodes.push(TreeNode {
                    name: name.clone(),
                    cls: cls.clone(),
                    parent: e.parent_node,
                    count: 0,
                    nearest: None,
                });
                kids.push(HashMap::new());
                keep.push(false);
            }
        }
        if node >= 0 {
            nodes[node as usize].count += 1;
            if is_base_part(&cls) || is_container(&cls) {
                let mut k = node;
                while k >= 0 && !keep[k as usize] {
                    keep[k as usize] = true;
                    k = nodes[k as usize].parent;
                }
                if let Some(from) = from
                    && let Some(p) = position_of(m, o, e.inst, &cls)
                {
                    let d = distance(from, p);
                    let n = &mut nodes[node as usize];
                    if n.nearest.is_none_or(|near| d < near) {
                        n.nearest = Some(d);
                    }
                }
            }
        }

        let pick = e.parent_pick.and_then(|pp| pp.kids.get(&name));
        let picked = pick.is_some_and(|p| p.picked);
        let folder = cls == "Folder";
        let mut in_match = e.in_match;
        if !in_match
            && out.matches.len() < lim.matches
            && ((picked && !folder) || e.in_picked_folder || matches(t, &name, &cls))
        {
            let prims = primitives_of(m, o, e.inst, &cls, lim.parts_per_match);
            if !prims.is_empty() {
                out.matches.push(Match {
                    inst: e.inst,
                    name,
                    primitives: prims,
                });
            }
            in_match = true;
        }
        if e.depth < lim.depth {
            push(
                m,
                o,
                &mut stack,
                e.inst,
                Entry {
                    inst: 0,
                    depth: e.depth + 1,
                    parent_node: node,
                    parent_pick: pick,
                    in_match,
                    in_picked_folder: picked && folder && !in_match,
                },
            );
        }
    }

    // Parents come before their children in `nodes`, so one pass keeps the tree consistent.
    let mut moved = vec![-1; nodes.len()];
    for (i, mut n) in nodes.into_iter().enumerate() {
        if !keep[i] {
            continue;
        }
        moved[i] = out.tree.len() as i32;
        if n.parent >= 0 {
            n.parent = moved[n.parent as usize];
        }
        out.tree.push(n);
    }
    out
}

// Where a primitive keeps its position and size, looked up once per frame rather than per part.
#[derive(Clone, Copy)]
struct PartFields {
    pos: i64,
    size: i64,
}

fn part_fields(o: &Offsets) -> PartFields {
    PartFields {
        pos: o["Primitive.Position"],
        size: o["Primitive.Size"],
    }
}

fn vec3_at(buf: &[u8], off: i64) -> Vec3 {
    let b = &buf[off as usize..off as usize + size_of::<Vec3>()];
    // SAFETY: b holds exactly one Vec3's bytes, and any bytes are a valid Vec3 (three f32s).
    unsafe { std::ptr::read_unaligned(b.as_ptr().cast::<Vec3>()) }
}

fn read_parts_at(m: &dyn Memory, f: PartFields, matched: &Match) -> Vec<Part> {
    let mut out = Vec::with_capacity(matched.primitives.len());
    let lo = f.pos.min(f.size);
    let span = f.pos.max(f.size) + 12 - lo;
    let mut buf = [0u8; 512];
    for &p in &matched.primitives {
        // both fields in one read while they sit close together (240 bytes apart on 5a21b87b), else one read each
        let part = if span <= buf.len() as i64 && m.read(p.wrapping_add(lo as usize), &mut buf[..span as usize]) {
            Part {
                pos: vec3_at(&buf, f.pos - lo),
                size: vec3_at(&buf, f.size - lo),
            }
        } else {
            let (pos, size) = (
                m.get::<Vec3>(p.wrapping_add(f.pos as usize)),
                m.get::<Vec3>(p.wrapping_add(f.size as usize)),
            );
            let (Some(pos), Some(size)) = (pos, size) else {
                continue;
            };
            Part { pos, size }
        };
        out.push(part);
    }
    out
}

/// Current position and size of each primitive that reads.
pub fn read_parts(m: &dyn Memory, o: &Offsets, matched: &Match) -> Vec<Part> {
    read_parts_at(m, part_fields(o), matched)
}

/// The whole scan of a running Roblox: its game, Players, and one walk of Workspace. Slow (one read per field of
/// thousands of instances), so the engine runs it on its own thread.
pub fn scan_roblox(p: &RobloxProcess, o: &Offsets, t: &Targets) -> Scan {
    let dm = data_model(p, o);
    let players = if dm != 0 {
        child_by_class(p, o, dm, "Players")
    } else {
        0
    };
    let mut s = scan_workspace(p, o, dm, t, local_root_position(p, o, players), &ScanLimits::default());
    s.players = players;
    s.game = if dm != 0 {
        (p as &dyn Memory).get::<i64>(o.at(dm, "DataModel.GameId")).unwrap_or(0)
    } else {
        0
    };
    s
}

/// Markers for a scan's matches from their current positions; cheap enough for every frame. None when the view is
/// unreadable (e.g. minimised).
pub fn frame_of(p: &RobloxProcess, o: &Offsets, s: &Scan, max_studs: i32) -> Option<Frame> {
    // Parts first, the camera last: boxes are projected with the newest camera, which moves the most between frames.
    let fields = part_fields(o);
    let parts: Vec<Vec<Part>> = s
        .matches
        .iter()
        .map(|matched| read_parts_at(p, fields, matched))
        .collect();
    let from = local_root_position(p, o, s.players);
    let view = read_screen_view(p, o, p.base())?;
    let mut f = Frame {
        size: view.size,
        markers: Vec::new(),
    };
    for (i, ps) in parts.iter().enumerate() {
        if let Some(mk) = marker_for(&view, ps, s.matches[i].name.clone(), from, max_studs) {
            f.markers.push(mk);
        }
    }
    Some(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use handy_sdk::roblox::screen::vec3;
    use handy_sdk::testing::{FakeMemory, fake_instance_offsets};

    // Test-only field offsets past the 0x40 FakeMemory::instance uses.
    fn offs() -> Offsets {
        let mut o = fake_instance_offsets();
        for (k, v) in [
            ("DataModel.Workspace", 0x48),
            ("BasePart.Primitive", 0x50),
            ("Player.ModelInstance", 0x58),
            ("Player.LocalPlayer", 0x60),
            ("Model.PrimaryPart", 0x68),
            ("Primitive.Position", 0x10),
            ("Primitive.Size", 0x20),
        ] {
            o.values.insert(k.into(), v);
        }
        o
    }

    fn part(m: &mut FakeMemory, name: &str, pos: Vec3) -> usize {
        let (p, prim, size) = (m.instance("Part", name, &[]), m.alloc(0x40), vec3(1., 1., 1.));
        m.put_ptr(p + 0x50, prim);
        m.put_val(prim + 0x10, &pos);
        m.put_val(prim + 0x20, &size);
        p
    }

    struct World {
        m: FakeMemory,
        dm: usize,
        players: usize,
    }

    fn strs(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_owned()).collect()
    }
    fn names(n: &[&str]) -> Targets {
        Targets {
            names: strs(n),
            ..Default::default()
        }
    }
    fn paths(p: &[&[&str]]) -> Targets {
        Targets {
            paths: p.iter().map(|p| strs(p)).collect(),
            ..Default::default()
        }
    }

    // Workspace: Chest (model with a Lid part), Folder/Chest (a part), Bob's character holding a Chest, my character.
    fn world() -> World {
        let mut m = FakeMemory::default();
        let lid = part(&mut m, "Lid", vec3(1., 2., 3.));
        let chest = m.instance("Model", "Chest", &[lid]);
        let loot_chest = part(&mut m, "Chest", vec3(4., 5., 6.));
        let folder = m.instance("Folder", "Loot", &[loot_chest]);
        let bob_chest = part(&mut m, "Chest", vec3(7., 8., 9.));
        let bob_char = m.instance("Model", "Bob", &[bob_chest]);
        let root = part(&mut m, "HumanoidRootPart", vec3(0., 0., 0.));
        let my_chest = part(&mut m, "Chest", vec3(0., 1., 0.));
        let my_char = m.instance("Model", "Me", &[root, my_chest]);
        m.put_ptr(my_char + 0x68, root);
        let ws = m.instance("Workspace", "Workspace", &[chest, folder, bob_char, my_char]);
        let (bob, me) = (m.instance("Player", "Bob", &[]), m.instance("Player", "Me", &[]));
        m.put_ptr(bob + 0x58, bob_char);
        m.put_ptr(me + 0x58, my_char);
        let players = m.instance("Players", "Players", &[bob, me]);
        m.put_ptr(players + 0x60, me);
        let dm = m.instance("DataModel", "Game", &[ws, players]);
        m.put_ptr(dm + 0x48, ws);
        World { m, dm, players }
    }

    fn found(w: &World, t: &Targets, lim: &ScanLimits) -> usize {
        scan_workspace(&w.m, &offs(), w.dm, t, None, lim).matches.len()
    }

    #[test]
    fn scanner_finds_by_name_and_skips_characters() {
        let w = world();
        let o = offs();
        let found = scan_workspace(&w.m, &o, w.dm, &names(&["chest"]), None, &ScanLimits::default()).matches;
        assert_eq!(found.len(), 2); // not the ones inside Bob's or my character
        assert!(found[0].name == "Chest" && found[0].primitives.len() == 1);
        let parts = read_parts(&w.m, &o, &found[0]);
        assert!(parts.len() == 1 && parts[0].pos.x == 1. && parts[0].pos.z == 3. && parts[0].size.y == 1.);
        assert_eq!(read_parts(&w.m, &o, &found[1])[0].pos.x, 4.);
    }

    #[test]
    fn scanner_finds_by_class_and_respects_limits() {
        let w = world();
        let def = ScanLimits::default();
        let folders = Targets {
            classes: strs(&["Folder"]),
            ..Default::default()
        };
        assert_eq!(found(&w, &folders, &def), 1);
        assert_eq!(found(&w, &names(&["Chest"]), &ScanLimits { depth: 1, ..def }), 1); // Loot/Chest is depth 2
        assert_eq!(found(&w, &names(&["Chest"]), &ScanLimits { matches: 1, ..def }), 1);
        assert_eq!(found(&w, &names(&["Chest", "Lid"]), &def), 2); // Lid is inside the Chest model: not matched again
        assert_eq!(found(&w, &Targets::default(), &def), 0);
        assert!(
            scan_workspace(&w.m, &offs(), 0, &names(&["Chest"]), None, &def)
                .tree
                .is_empty()
        );
    }

    #[test]
    fn scanner_matches_picked_paths() {
        let w = world();
        let o = offs();
        let def = ScanLimits::default();
        let only = |t: &Targets| {
            let m = scan_workspace(&w.m, &o, w.dm, t, None, &def).matches;
            if m.len() == 1 {
                read_parts(&w.m, &o, &m[0])[0].pos.x
            } else {
                -1.
            }
        };
        assert_eq!(only(&paths(&[&["Chest"]])), 1.); // the Chest model, not Loot's Chest
        assert_eq!(only(&paths(&[&["Loot", "Chest"]])), 4.); // only Loot's Chest
        assert_eq!(only(&paths(&[&["Loot"]])), 4.); // a picked folder: what's directly in it
        assert_eq!(only(&paths(&[&["Chest", "Lid"]])), 1.);
        assert_eq!(found(&w, &paths(&[&["Chest"], &["Chest", "Lid"]]), &def), 1); // nothing inside a match again
        // exact names; never characters
        assert_eq!(found(&w, &paths(&[&["chest"], &["Nope"], &["Bob", "Chest"]]), &def), 0);
    }

    #[test]
    fn scanner_builds_the_tree() {
        let w = world();
        let o = offs();
        let def = ScanLimits::default();
        // also while nothing is targeted; not characters or their parts
        let tree = scan_workspace(&w.m, &o, w.dm, &Targets::default(), Some(vec3(0., 0., 0.)), &def).tree;
        assert_eq!(tree.len(), 5);
        assert!(tree[0].name == "Workspace" && tree[0].parent == -1);
        assert!(tree[1].name == "Chest" && tree[1].cls == "Model" && tree[1].parent == 0 && tree[1].nearest.is_none()); // no PrimaryPart
        assert!(tree[2].name == "Lid" && tree[2].parent == 1 && tree[2].nearest.is_some());
        assert!(tree[3].name == "Loot" && tree[3].cls == "Folder" && tree[3].parent == 0);
        assert!(
            tree[4].name == "Chest" && tree[4].parent == 3 && tree[4].nearest.is_some_and(|d| d > 8.77 && d < 8.78)
        ); // |(4, 5, 6)|
        // no position of yours: no distances
        assert!(
            scan_workspace(&w.m, &o, w.dm, &Targets::default(), None, &def).tree[2]
                .nearest
                .is_none()
        );
        let three = ScanLimits { nodes: 3, ..def };
        assert_eq!(
            scan_workspace(&w.m, &o, w.dm, &Targets::default(), None, &three)
                .tree
                .len(),
            3
        );
    }

    #[test]
    fn scanner_tree_groups_same_names_and_drops_what_cannot_be_drawn() {
        let mut m = FakeMemory::default();
        let o = offs();
        // Workspace { Coins (Folder) { Coin, Coin, Coin (parts) }, Sounds (Folder) { Boom (Sound) }, Boom (Sound) }
        let coin_parts = [
            part(&mut m, "Coin", vec3(1., 0., 0.)),
            part(&mut m, "Coin", vec3(2., 0., 0.)),
            part(&mut m, "Coin", vec3(3., 0., 0.)),
        ];
        let coins = m.instance("Folder", "Coins", &coin_parts);
        let boom = m.instance("Sound", "Boom", &[]);
        let sounds = m.instance("Folder", "Sounds", &[boom]);
        let boom2 = m.instance("Sound", "Boom", &[]);
        let ws = m.instance("Workspace", "Workspace", &[coins, sounds, boom2]);
        let dm = m.instance("DataModel", "Game", &[ws]);
        m.put_ptr(dm + 0x48, ws);
        let tree = scan_workspace(
            &m,
            &o,
            dm,
            &Targets::default(),
            Some(vec3(0., 0., 0.)),
            &ScanLimits::default(),
        )
        .tree;
        assert_eq!(tree.len(), 4); // Workspace, Coins, Coin, Sounds (an empty-looking folder still holds a place)
        assert!(tree[2].name == "Coin" && tree[2].count == 3 && tree[2].nearest == Some(1.));
        assert!(tree[3].name == "Sounds" && tree[3].parent == 0);
    }

    #[test]
    fn scanner_local_root() {
        let w = world();
        let o = offs();
        let r = local_root_position(&w.m, &o, w.players);
        assert!(r.is_some_and(|r| r.x == 0. && r.y == 0.));
        assert!(local_root_position(&w.m, &o, 0).is_none());
    }

    // Needs a running Roblox client + network.
    #[test]
    #[ignore]
    fn live_view_and_scan() {
        use handy_sdk::app::updater_thread::{AppStatus, Updater, production_smoke_test};
        use handy_sdk::roblox::http::win_http_get;
        use handy_sdk::util::paths::{app_data_dir, set_app_name};
        use handy_sdk::util::thread::StopThread;
        use std::sync::Arc;

        set_app_name("handy's esp"); // the real offset cache, like the app
        let updater = Arc::new(Updater::new(
            Arc::new(win_http_get),
            production_smoke_test,
            app_data_dir(),
            &crate::app::offset_keys::required_keys(),
        ));
        let u = updater.clone();
        let _thread = StopThread::spawn(move |stop| u.run(stop));
        let mut s = None;
        for _ in 0..600 {
            if updater.status() == AppStatus::Ready {
                s = updater.session();
            }
            if s.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let s = s.expect("Ready within a minute");
        let view = read_screen_view(&s.proc, &s.offsets, s.proc.base());
        assert!(view.is_some());
        if let Some(view) = view {
            assert!(view.m.iter().all(|f| f.is_finite()));
            println!("  render size {:.0} x {:.0}", view.size.x, view.size.y);
        }
        // most places have a SpawnLocation, a part class; printed, not checked, since some places have none
        let spawns = Targets {
            classes: strs(&["SpawnLocation"]),
            ..Default::default()
        };
        let scan = scan_roblox(&s.proc, &s.offsets, &spawns);
        assert_ne!(scan.game, 0);
        println!(
            "  game {}, SpawnLocations: {}, places to pick from: {}",
            scan.game,
            scan.matches.len(),
            scan.tree.len()
        );
        let frame = frame_of(&s.proc, &s.offsets, &scan, 0);
        assert!(frame.is_some());
        if let Some(frame) = frame {
            println!("  on screen: {}", frame.markers.len());
        }
    }
}
