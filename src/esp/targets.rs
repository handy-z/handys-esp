/// Names from Workspace down to an instance, e.g. {"Map", "Coins", "Coin"}: every instance at that place (exact names).
pub type Path = Vec<String>;

/// What to draw: instances at a picked path (a picked Folder means each thing directly in it), or whose Name or
/// ClassName is listed (those two compared ignoring ASCII case).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Targets {
    pub names: Vec<String>,
    pub classes: Vec<String>,
    pub paths: Vec<Path>,
}

impl Targets {
    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.classes.is_empty() && self.paths.is_empty()
    }
}

fn listed(list: &[String], s: &str) -> bool {
    list.iter().any(|e| e.eq_ignore_ascii_case(s))
}

/// "Chest, Coin,,Tool " -> {"Chest", "Coin", "Tool"}.
pub fn parse_list(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(|item| item.trim_matches([' ', '\t']))
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// "Chest, Coin"
pub fn join_list(list: &[String]) -> String {
    list.join(", ")
}

/// Paths as one ini value: names joined by '/', paths by '|', and '\' escaping any of the three inside a name.
pub fn join_paths(paths: &[Path]) -> String {
    let mut out = String::new();
    for p in paths {
        if !out.is_empty() {
            out.push('|');
        }
        for (i, name) in p.iter().enumerate() {
            if i > 0 {
                out.push('/');
            }
            for c in name.chars() {
                if matches!(c, '\\' | '/' | '|') {
                    out.push('\\');
                }
                out.push(c);
            }
        }
    }
    out
}

pub fn parse_paths(s: &str) -> Vec<Path> {
    let mut out = Vec::new();
    let mut p: Path = vec![String::new()];
    let mut end_path = |p: &mut Path| {
        let done = std::mem::replace(p, vec![String::new()]);
        if !(done.len() == 1 && done[0].is_empty()) {
            out.push(done);
        }
    };
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek().is_some() => p.last_mut().unwrap().push(chars.next().unwrap()),
            '/' => p.push(String::new()),
            '|' => end_path(&mut p),
            _ => p.last_mut().unwrap().push(c),
        }
    }
    end_path(&mut p);
    out
}

/// "Map › Coins › Coin", for people.
pub fn show_path(p: &[String]) -> String {
    p.join(" \u{203A} ")
}

/// By names= or classes=.
pub fn matches(t: &Targets, name: &str, cls: &str) -> bool {
    listed(&t.names, name) || listed(&t.classes, cls)
}

pub fn has_path(t: &Targets, p: &[String]) -> bool {
    t.paths.iter().any(|q| q.as_slice() == p)
}

/// t with `p` added at the end (when missing) or removed.
pub fn with_path(mut t: Targets, p: &[String], on: bool) -> Targets {
    if on && !has_path(&t, p) {
        t.paths.push(p.to_vec());
    }
    if !on {
        t.paths.retain(|q| q.as_slice() != p);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn targets_parse_trims_and_drops_empty() {
        assert_eq!(parse_list(" Chest, Coin,,\tTool ,"), v(&["Chest", "Coin", "Tool"]));
        assert!(parse_list("").is_empty());
        assert!(parse_list(" , ").is_empty());
    }

    #[test]
    fn targets_join_round_trips() {
        assert_eq!(join_list(&v(&["Chest", "Coin"])), "Chest, Coin");
        assert_eq!(parse_list(&join_list(&v(&["Chest", "Coin"]))), v(&["Chest", "Coin"]));
    }

    #[test]
    fn targets_match_name_or_class_ignoring_case() {
        let t = Targets {
            names: v(&["Chest"]),
            classes: v(&["Tool"]),
            ..Default::default()
        };
        assert!(matches(&t, "chest", "Model"));
        assert!(matches(&t, "Sword", "TOOL"));
        assert!(!matches(&t, "Chests", "Model"));
        assert!(!matches(&Targets::default(), "Chest", "Tool"));
    }

    #[test]
    fn targets_paths_round_trip_with_escapes() {
        let paths = vec![v(&["Map", "Coins", "Coin"]), v(&["a/b", "c|d", "e\\f"]), v(&["Loot"])];
        assert_eq!(join_paths(&paths), "Map/Coins/Coin|a\\/b/c\\|d/e\\\\f|Loot");
        assert_eq!(parse_paths(&join_paths(&paths)), paths);
        assert!(parse_paths("").is_empty());
        assert_eq!(parse_paths("|Loot|").len(), 1);
        assert_eq!(show_path(&v(&["Map", "Coins"])), "Map \u{203A} Coins");
    }

    #[test]
    fn targets_with_path_adds_once_and_removes() {
        let t = Targets {
            paths: vec![v(&["Map", "Coin"])],
            ..Default::default()
        };
        assert_eq!(
            with_path(t.clone(), &v(&["Loot"]), true).paths,
            vec![v(&["Map", "Coin"]), v(&["Loot"])]
        );
        assert_eq!(with_path(t.clone(), &v(&["Map", "Coin"]), true), t);
        assert!(with_path(t.clone(), &v(&["Map", "Coin"]), false).is_empty());
        assert!(
            has_path(&t, &v(&["Map", "Coin"])) && !has_path(&t, &v(&["Map"])) && !has_path(&t, &v(&["map", "coin"]))
        );
    }
}
