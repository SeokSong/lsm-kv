// the subset of YAML iRacing writes in session info: maps, lists of maps, plain scalars.
// a real YAML library would be overkill, and iRacing's text isn't always valid YAML (driver
// and team names aren't quoted), so this reads it line by line and never fails: a line it
// can't make sense of is skipped. one quirk handled: list items sit at the same indent as
// their parent key
//
//  SplitTimeInfo:
//   Sectors:
//   - SectorNum: 0
//     SectorStartPct: 0.000000

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Scalar(String),
    Map(Vec<(String, Node)>),
    List(Vec<Node>),
}

impl Node {
    // a value by path: "WeekendInfo.TrackID"
    pub fn get(&self, path: &str) -> Option<&Node> {
        path.split('.').try_fold(self, |node, key| match node {
            Node::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        })
    }

    pub fn str(&self, path: &str) -> Option<&str> {
        match self.get(path)? {
            Node::Scalar(value) => Some(value),
            _ => None,
        }
    }

    pub fn list(&self, path: &str) -> &[Node] {
        match self.get(path) {
            Some(Node::List(items)) => items,
            _ => &[],
        }
    }
}

struct Line<'a> {
    indent: usize,
    // the text after the indent, a leading "- " still attached
    text: &'a str,
}

pub fn parse(text: &str) -> Node {
    let lines: Vec<Line<'_>> = text
        .lines()
        .filter(|line| !line.trim().is_empty() && line.trim() != "---" && line.trim() != "...")
        .map(|line| {
            let trimmed: &str = line.trim_start_matches(' ');
            Line {
                indent: line.len() - trimmed.len(),
                text: trimmed.trim_end(),
            }
        })
        .collect();
    let mut at: usize = 0;
    parse_map(&lines, &mut at, 0)
}

// entries at exactly `indent`, until a line is less indented (or a list item at this indent)
fn parse_map(lines: &[Line<'_>], at: &mut usize, indent: usize) -> Node {
    let mut entries: Vec<(String, Node)> = Vec::new();
    while *at < lines.len() {
        let line: &Line<'_> = &lines[*at];
        if line.indent < indent || (line.indent == indent && line.text.starts_with("- ")) {
            break;
        }
        if line.indent > indent {
            // deeper than expected and not claimed by a key: skip it
            *at += 1;
            continue;
        }
        *at += 1;
        let Some((key, value)) = split_entry(line.text) else {
            continue;
        };
        if !value.is_empty() {
            entries.push((key.to_string(), Node::Scalar(value.to_string())));
            continue;
        }
        // no value: a nested block follows, deeper, or a list at this same indent
        let child: Node = match lines.get(*at) {
            Some(next) if next.indent == indent && next.text.starts_with("- ") => {
                parse_list(lines, at, indent)
            }
            Some(next) if next.indent > indent => {
                let child_indent: usize = next.indent;
                if next.text.starts_with("- ") {
                    parse_list(lines, at, child_indent)
                } else {
                    parse_map(lines, at, child_indent)
                }
            }
            _ => Node::Scalar(String::new()),
        };
        entries.push((key.to_string(), child));
    }
    Node::Map(entries)
}

// "- key: value" items at `indent`, each item's other keys at indent + 2
fn parse_list(lines: &[Line<'_>], at: &mut usize, indent: usize) -> Node {
    let mut items: Vec<Node> = Vec::new();
    while *at < lines.len() {
        let line: &Line<'_> = &lines[*at];
        if line.indent != indent || !line.text.starts_with("- ") {
            break;
        }
        *at += 1;
        let mut entries: Vec<(String, Node)> = Vec::new();
        if let Some((key, value)) = split_entry(&line.text[2..]) {
            entries.push((key.to_string(), Node::Scalar(value.to_string())));
        }
        if let Node::Map(rest) = parse_map(lines, at, indent + 2) {
            entries.extend(rest);
        }
        items.push(Node::Map(entries));
    }
    Node::List(items)
}

// "key: value" -> (key, value without quotes). the first ": " splits, values may hold colons
fn split_entry(text: &str) -> Option<(&str, &str)> {
    let (key, value) = match text.split_once(": ") {
        Some((key, value)) => (key, value.trim()),
        None => (text.strip_suffix(':')?, ""),
    };
    let value: &str = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    Some((key.trim(), value))
}
