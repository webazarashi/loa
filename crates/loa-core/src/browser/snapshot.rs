//! Parser for agent-browser accessibility snapshots, used to resolve role/name locators
//! to `@eN` refs deterministically (exact name match, optional ancestor scope).

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Node {
    pub role: String,
    pub name: Option<String>,
    pub reference: Option<String>,
    pub attrs: Vec<String>,
    pub value: Option<String>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    pub roots: Vec<Node>,
}

/// Roles that a `label:` locator may resolve to (form controls named by their label).
pub const LABELLED_ROLES: [&str; 10] = [
    "textbox",
    "combobox",
    "checkbox",
    "radio",
    "spinbutton",
    "searchbox",
    "listbox",
    "switch",
    "slider",
    "button",
];

impl Snapshot {
    pub fn parse(text: &str) -> Snapshot {
        let mut roots: Vec<Node> = Vec::new();
        // stack of (indent, path into roots)
        let mut stack: Vec<(usize, Vec<usize>)> = Vec::new();
        for raw in text.lines() {
            let indent = raw.len() - raw.trim_start().len();
            let line = raw.trim_start();
            let Some(body) = line.strip_prefix("- ") else {
                continue;
            };
            let node = parse_line(body);
            while let Some((ind, _)) = stack.last() {
                if *ind >= indent {
                    stack.pop();
                } else {
                    break;
                }
            }
            let path = match stack.last() {
                None => {
                    roots.push(node);
                    vec![roots.len() - 1]
                }
                Some((_, parent_path)) => {
                    let parent = node_at_mut(&mut roots, parent_path);
                    parent.children.push(node);
                    let mut p = parent_path.clone();
                    p.push(parent.children.len() - 1);
                    p
                }
            };
            stack.push((indent, path));
        }
        Snapshot { roots }
    }

    pub fn walk(&self) -> Vec<&Node> {
        let mut out = Vec::new();
        for r in &self.roots {
            collect(r, &mut out);
        }
        out
    }

    /// All nodes matching role (case-insensitive) and exact name, optionally under `scope`.
    pub fn find<'a>(
        &'a self,
        role: Option<&str>,
        name: &str,
        scope: Option<&'a Node>,
    ) -> Vec<&'a Node> {
        let mut all = Vec::new();
        match scope {
            Some(s) => {
                for c in &s.children {
                    collect(c, &mut all);
                }
            }
            None => {
                for r in &self.roots {
                    collect(r, &mut all);
                }
            }
        }
        all.into_iter()
            .filter(|n| {
                n.name.as_deref() == Some(name)
                    && match role {
                        Some(r) => n.role.eq_ignore_ascii_case(r),
                        None => LABELLED_ROLES
                            .iter()
                            .any(|lr| n.role.eq_ignore_ascii_case(lr)),
                    }
            })
            .collect()
    }

    /// Text of every node, for redacted failure reports.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for r in &self.roots {
            render(r, 0, &mut out);
        }
        out
    }
}

fn render(n: &Node, depth: usize, out: &mut String) {
    out.push_str(&"  ".repeat(depth));
    out.push_str("- ");
    out.push_str(&n.role);
    if let Some(name) = &n.name {
        out.push_str(&format!(" \"{name}\""));
    }
    if let Some(v) = &n.value {
        out.push_str(&format!(": {v}"));
    }
    out.push('\n');
    for c in &n.children {
        render(c, depth + 1, out);
    }
}

fn collect<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
    out.push(n);
    for c in &n.children {
        collect(c, out);
    }
}

fn node_at_mut<'a>(roots: &'a mut [Node], path: &[usize]) -> &'a mut Node {
    let mut cur = &mut roots[path[0]];
    for &i in &path[1..] {
        cur = &mut cur.children[i];
    }
    cur
}

/// `role "name" [attr, ref=e1]: value`
fn parse_line(body: &str) -> Node {
    let mut node = Node::default();
    let mut rest = body;
    // role: up to first space or end
    let role_end = rest.find([' ', ':']).unwrap_or(rest.len());
    node.role = rest[..role_end].to_string();
    rest = &rest[role_end..];
    rest = rest.trim_start();
    if let Some(after) = rest.strip_prefix('"') {
        // name with escaped quotes
        let mut name = String::new();
        let mut chars = after.char_indices();
        let mut end = after.len();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    if let Some((_, n)) = chars.next() {
                        name.push(n);
                    }
                }
                '"' => {
                    end = i + 1;
                    break;
                }
                _ => name.push(c),
            }
        }
        node.name = Some(name);
        rest = after[end..].trim_start();
    }
    if let Some(after) = rest.strip_prefix('[')
        && let Some(close) = after.find(']')
    {
        for a in after[..close].split(',') {
            let a = a.trim();
            if let Some(r) = a.strip_prefix("ref=") {
                node.reference = Some(r.to_string());
            } else if !a.is_empty() {
                node.attrs.push(a.to_string());
            }
        }
        rest = after[close + 1..].trim_start();
    }
    if let Some(v) = rest.strip_prefix(':') {
        let v = v.trim();
        if !v.is_empty() {
            node.value = Some(v.to_string());
        }
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "- heading \"プロバイダー\" [level=1, ref=e1]\n- LabelText\n  - StaticText \"チャネル名\"\n- textbox \"チャネル名\" [ref=e2]: hello\n- row \"acme\"\n  - StaticText \"acme\"\n  - button \"削除\" [ref=e5]\n- row \"beta\"\n  - StaticText \"beta\"\n  - button \"削除\" [ref=e6]\n- button \"作成\" [ref=e4]";

    #[test]
    fn parses_tree_and_refs() {
        let s = Snapshot::parse(SAMPLE);
        assert_eq!(s.roots.len(), 6);
        assert_eq!(s.roots[0].reference.as_deref(), Some("e1"));
        assert_eq!(s.roots[0].attrs, vec!["level=1".to_string()]);
        assert_eq!(s.roots[2].value.as_deref(), Some("hello"));
        assert_eq!(s.roots[3].children.len(), 2);
    }

    #[test]
    fn scoped_find_disambiguates_rows() {
        let s = Snapshot::parse(SAMPLE);
        assert_eq!(s.find(Some("button"), "削除", None).len(), 2);
        let row = s.find(Some("row"), "beta", None)[0];
        let hits = s.find(Some("button"), "削除", Some(row));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].reference.as_deref(), Some("e6"));
    }

    #[test]
    fn label_find_matches_form_controls_only() {
        let s = Snapshot::parse(SAMPLE);
        let hits = s.find(None, "チャネル名", None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].role, "textbox");
    }

    #[test]
    fn escaped_quotes_in_names() {
        let s = Snapshot::parse("- button \"say \\\"hi\\\"\" [ref=e9]");
        assert_eq!(s.roots[0].name.as_deref(), Some("say \"hi\""));
        assert_eq!(s.roots[0].reference.as_deref(), Some("e9"));
    }
}
