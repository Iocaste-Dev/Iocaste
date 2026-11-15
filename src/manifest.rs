//! Reading and editing `ioc.toml` (a small, strict subset of TOML) and `ioc.lock`.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    /// `{ path = "../x", version = "^1" }`
    Table(BTreeMap<String, String>),
    /// numbers, booleans, arrays: kept as written
    Raw(String),
}

#[derive(Debug, Default, Clone)]
pub struct Manifest {
    pub sections: BTreeMap<String, Vec<(String, Value)>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// `{ path = "../mathx" }`
    Path(String),
    /// `"^0.1"`: comes from the cluster registry
    Registry(String),
}

#[derive(Debug, Clone)]
pub struct ClusterReq {
    pub name: String,
    pub source: Source,
    /// optional extra version requirement for path clusters
    pub version: Option<String>,
}

/// Cut a trailing `# comment` that is not inside a string.
fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in line.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
        } else if c == '#' {
            return &line[..i];
        }
    }
    line
}

fn parse_string(s: &str) -> Option<String> {
    let s = s.trim();
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                _ => return None,
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn parse_key(s: &str) -> Option<String> {
    let s = s.trim();
    if s.starts_with('"') {
        parse_string(s)
    } else if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
        Some(s.to_string())
    } else {
        None
    }
}

fn parse_value(s: &str) -> Option<Value> {
    let s = s.trim();
    if s.starts_with('"') {
        return parse_string(s).map(Value::Str);
    }
    if let Some(body) = s.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
        let mut t = BTreeMap::new();
        let mut rest = body.trim();
        while !rest.is_empty() {
            let (k, after) = rest.split_once('=')?;
            let k = parse_key(k)?;
            let after = after.trim_start();
            if !after.starts_with('"') {
                return None;
            }
            // find the closing quote
            let mut end = None;
            let mut esc = false;
            for (i, c) in after.char_indices().skip(1) {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    end = Some(i);
                    break;
                }
            }
            let end = end?;
            t.insert(k, parse_string(&after[..=end])?);
            rest = after[end + 1..].trim_start();
            if let Some(r) = rest.strip_prefix(',') {
                rest = r.trim_start();
            } else if !rest.is_empty() {
                return None;
            }
        }
        return Some(Value::Table(t));
    }
    if s.is_empty() {
        None
    } else {
        Some(Value::Raw(s.to_string()))
    }
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let mut m = Manifest::default();
        let mut section = String::new();
        for (i, raw) in text.lines().enumerate() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(h) = line.strip_prefix('[') {
                let name = h.strip_suffix(']').ok_or(format!("line {}: unclosed `[`", i + 1))?;
                section = name.trim().to_string();
                m.sections.entry(section.clone()).or_default();
                continue;
            }
            let (k, v) = line.split_once('=').ok_or(format!("line {}: expected `key = value`", i + 1))?;
            let key = parse_key(k).ok_or(format!("line {}: bad key `{}`", i + 1, k.trim()))?;
            let val = parse_value(v).ok_or(format!("line {}: bad value `{}`", i + 1, v.trim()))?;
            m.sections.entry(section.clone()).or_default().push((key, val));
        }
        Ok(m)
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&Value> {
        self.sections.get(section)?.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_str(&self, section: &str, key: &str) -> Option<String> {
        match self.get(section, key)? {
            Value::Str(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn name(&self) -> Option<String> {
        self.get_str("project", "name")
    }

    pub fn version(&self) -> Option<String> {
        self.get_str("project", "version")
    }

    pub fn clusters(&self) -> Result<Vec<ClusterReq>, String> {
        let mut out = Vec::new();
        for (k, v) in self.sections.get("clusters").map(|v| v.as_slice()).unwrap_or(&[]) {
            match v {
                Value::Str(req) => out.push(ClusterReq { name: k.clone(), source: Source::Registry(req.clone()), version: None }),
                Value::Table(t) => {
                    if let Some(p) = t.get("path") {
                        out.push(ClusterReq { name: k.clone(), source: Source::Path(p.clone()), version: t.get("version").cloned() });
                    } else if let Some(r) = t.get("version") {
                        out.push(ClusterReq { name: k.clone(), source: Source::Registry(r.clone()), version: None });
                    } else {
                        return Err(format!("cluster `{}`: expected a version string or {{ path = \"...\" }}", k));
                    }
                }
                Value::Raw(_) => return Err(format!("cluster `{}`: expected a version string or {{ path = \"...\" }}", k)),
            }
        }
        Ok(out)
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn is_header(line: &str) -> bool {
    strip_comment(line).trim().starts_with('[')
}

fn entry_key(line: &str) -> Option<String> {
    let l = strip_comment(line).trim();
    if l.is_empty() || l.starts_with('[') {
        return None;
    }
    parse_key(l.split_once('=')?.0)
}

/// Add or replace `name = value` in the `[clusters]` section (created if missing). `value` is TOML text.
pub fn set_cluster(text: &str, name: &str, value_toml: &str) -> String {
    let key = if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') { name.to_string() } else { quote(name) };
    let new_line = format!("{} = {}", key, value_toml);
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let start = lines.iter().position(|l| strip_comment(l).trim() == "[clusters]");
    match start {
        None => {
            if lines.last().map_or(false, |l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("[clusters]".into());
            lines.push(new_line);
        }
        Some(s) => {
            let end = lines.iter().enumerate().skip(s + 1).find(|(_, l)| is_header(l)).map(|(i, _)| i).unwrap_or(lines.len());
            if let Some(i) = (s + 1..end).find(|&i| entry_key(&lines[i]).as_deref() == Some(name)) {
                lines[i] = new_line;
            } else {
                // after the last real entry of the section (keep trailing comments/blank lines below it)
                let at = (s + 1..end).rev().find(|&i| entry_key(&lines[i]).is_some()).map(|i| i + 1).unwrap_or(s + 1);
                lines.insert(at, new_line);
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Remove `name` from `[clusters]`. `None` if it was not there.
pub fn remove_cluster(text: &str, name: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let s = lines.iter().position(|l| strip_comment(l).trim() == "[clusters]")?;
    let end = lines.iter().enumerate().skip(s + 1).find(|(_, l)| is_header(l)).map(|(i, _)| i).unwrap_or(lines.len());
    let i = (s + 1..end).find(|&i| entry_key(lines[i]).as_deref() == Some(name))?;
    let mut out: Vec<&str> = lines;
    out.remove(i);
    let mut s = out.join("\n");
    s.push('\n');
    Some(s)
}

/// Stable 64-bit FNV-1a hash, printed as `fnv:<hex>`.
pub fn fnv(parts: &[(&str, &[u8])]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut feed = |b: &[u8]| {
        for x in b {
            h ^= *x as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    for (name, data) in parts {
        feed(name.as_bytes());
        feed(&[0]);
        feed(data);
        feed(&[0xff]);
    }
    format!("fnv:{:016x}", h)
}

#[derive(Debug, Clone, PartialEq)]
pub struct LockEntry {
    pub name: String,
    pub version: String,
    pub source: String,
    pub path: String,
    pub hash: String,
}

pub fn write_lock(entries: &[LockEntry]) -> String {
    let mut s = String::from("# ioc.lock: written by `ioc install`. Do not edit by hand.\n");
    for e in entries {
        s.push_str(&format!(
            "\n[[cluster]]\nname = {}\nversion = {}\nsource = {}\npath = {}\nhash = {}\n",
            quote(&e.name),
            quote(&e.version),
            quote(&e.source),
            quote(&e.path),
            quote(&e.hash)
        ));
    }
    s
}

pub fn read_lock(text: &str) -> Vec<LockEntry> {
    let mut out: Vec<LockEntry> = Vec::new();
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line == "[[cluster]]" {
            out.push(LockEntry { name: String::new(), version: String::new(), source: String::new(), path: String::new(), hash: String::new() });
        } else if let (Some(cur), Some((k, v))) = (out.last_mut(), line.split_once('=')) {
            let v = parse_string(v).unwrap_or_default();
            match k.trim() {
                "name" => cur.name = v,
                "version" => cur.version = v,
                "source" => cur.source = v,
                "path" => cur.path = v,
                "hash" => cur.hash = v,
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# manifest\n[project]\nname = \"demo\"   # trailing\nversion = \"0.1.0\"\n\n[clusters]\n# \"ioc.api\" = \"^0.1\"\nmathx = { path = \"../mathx\" }\n\"ioc.game\" = \"^0.2\"\n\n[extensions]\n";

    #[test]
    fn parses_sections_and_clusters() {
        let m = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(m.name().as_deref(), Some("demo"));
        let c = m.clusters().unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].name, "mathx");
        assert_eq!(c[0].source, Source::Path("../mathx".into()));
        assert_eq!(c[1].name, "ioc.game");
        assert_eq!(c[1].source, Source::Registry("^0.2".into()));
    }

    #[test]
    fn syntax_errors_have_line_numbers() {
        let e = Manifest::parse("[project\nname = 1").unwrap_err();
        assert!(e.contains("line 1"), "{}", e);
        let e = Manifest::parse("[project]\nname \"x\"").unwrap_err();
        assert!(e.contains("line 2"), "{}", e);
    }

    #[test]
    fn hash_inside_a_string_is_not_a_comment() {
        let m = Manifest::parse("[project]\nname = \"a#b\"").unwrap();
        assert_eq!(m.name().as_deref(), Some("a#b"));
    }

    #[test]
    fn add_replace_remove() {
        let t = set_cluster(SAMPLE, "ioc.api", "\"^0.1\"");
        assert!(t.contains("\"ioc.api\" = \"^0.1\"\n\n[extensions]"), "{}", t);
        let m = Manifest::parse(&t).unwrap();
        assert_eq!(m.clusters().unwrap().len(), 3);
        let t2 = set_cluster(&t, "ioc.game", "\"^0.3\"");
        assert!(t2.contains("\"ioc.game\" = \"^0.3\""));
        assert_eq!(Manifest::parse(&t2).unwrap().clusters().unwrap().len(), 3);
        let t3 = remove_cluster(&t2, "mathx").unwrap();
        assert!(!t3.contains("mathx"));
        assert!(remove_cluster(&t3, "nope").is_none());
    }

    #[test]
    fn adds_section_when_missing() {
        let t = set_cluster("[project]\nname = \"x\"\n", "foo", "\"1.0.0\"");
        assert!(t.ends_with("[clusters]\nfoo = \"1.0.0\"\n"), "{}", t);
    }

    #[test]
    fn lock_round_trip() {
        let e = vec![LockEntry { name: "a".into(), version: "1.0.0".into(), source: "path:../a".into(), path: "/x/a".into(), hash: "fnv:00".into() }];
        assert_eq!(read_lock(&write_lock(&e)), e);
    }

    #[test]
    fn hash_changes_with_content() {
        assert_ne!(fnv(&[("a", b"1")]), fnv(&[("a", b"2")]));
        assert_eq!(fnv(&[("a", b"1")]), fnv(&[("a", b"1")]));
    }
}
