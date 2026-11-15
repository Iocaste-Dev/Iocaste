//! Versions like `0.0.5-alpha` and requirements like `^0.1`.

use std::cmp::Ordering;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// `alpha`, `beta`, `rc1`... (a version with a label is older than the same version without)
    pub pre: Option<String>,
}

fn pre_rank(p: &Option<String>) -> (u8, String) {
    match p {
        None => (9, String::new()),
        Some(s) => {
            let l = s.to_ascii_lowercase();
            let rank = if l.starts_with("alpha") {
                1
            } else if l.starts_with("beta") {
                2
            } else if l.starts_with("rc") {
                3
            } else {
                0
            };
            (rank, l)
        }
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(o.major, o.minor, o.patch))
            .then_with(|| pre_rank(&self.pre).cmp(&pre_rank(&o.pre)))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(p) = &self.pre {
            write!(f, "-{}", p)?;
        }
        Ok(())
    }
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim().trim_start_matches('v');
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p.to_string())),
            Some(_) => return None,
            None => (s, None),
        };
        let mut it = core.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next().map_or(Some(0), |x| x.parse().ok())?;
        let patch = it.next().map_or(Some(0), |x| x.parse().ok())?;
        if it.next().is_some() {
            return None;
        }
        Some(Version { major, minor, patch, pre })
    }
}

/// Does `v` satisfy the requirement `req`? Supports `*`, `1.2.3` (exact), `^1.2`, `~1.2`, `>=1.2`.
/// Returns `None` when the requirement itself is malformed.
pub fn satisfies(req: &str, v: &Version) -> Option<bool> {
    let req = req.trim();
    if req == "*" || req.is_empty() {
        return Some(true);
    }
    if let Some(r) = req.strip_prefix(">=") {
        return Some(*v >= Version::parse(r)?);
    }
    if let Some(r) = req.strip_prefix('^') {
        let lo = Version::parse(r)?;
        let hi = if lo.major > 0 {
            Version { major: lo.major + 1, minor: 0, patch: 0, pre: None }
        } else if lo.minor > 0 {
            Version { major: 0, minor: lo.minor + 1, patch: 0, pre: None }
        } else {
            Version { major: 0, minor: 0, patch: lo.patch + 1, pre: None }
        };
        let lo = Version { pre: None, ..lo };
        let vv = Version { pre: None, ..v.clone() };
        return Some(vv >= lo && vv < hi);
    }
    if let Some(r) = req.strip_prefix('~') {
        let lo = Version::parse(r)?;
        let hi = Version { major: lo.major, minor: lo.minor + 1, patch: 0, pre: None };
        let lo = Version { pre: None, ..lo };
        let vv = Version { pre: None, ..v.clone() };
        return Some(vv >= lo && vv < hi);
    }
    Some(*v == Version::parse(req)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parse_and_show() {
        assert_eq!(v("0.0.5-alpha").to_string(), "0.0.5-alpha");
        assert_eq!(v("v1.2").to_string(), "1.2.0");
        assert!(Version::parse("1.x").is_none());
        assert!(Version::parse("1.2.3.4").is_none());
    }

    #[test]
    fn ordering() {
        assert!(v("0.0.6-alpha") > v("0.0.5-alpha"));
        assert!(v("0.1.0-alpha") > v("0.0.9"));
        assert!(v("1.0.0") > v("1.0.0-rc1"));
        assert!(v("1.0.0-rc1") > v("1.0.0-beta"));
        assert!(v("1.0.0-beta") > v("1.0.0-alpha"));
    }

    #[test]
    fn requirements() {
        assert_eq!(satisfies("^0.1", &v("0.1.9")), Some(true));
        assert_eq!(satisfies("^0.1", &v("0.2.0")), Some(false));
        assert_eq!(satisfies("^1.2", &v("1.9.0")), Some(true));
        assert_eq!(satisfies("^1.2", &v("2.0.0")), Some(false));
        assert_eq!(satisfies("~1.2", &v("1.2.7")), Some(true));
        assert_eq!(satisfies("~1.2", &v("1.3.0")), Some(false));
        assert_eq!(satisfies(">=0.0.3", &v("0.0.5-alpha")), Some(true));
        assert_eq!(satisfies("1.0.0", &v("1.0.0")), Some(true));
        assert_eq!(satisfies("*", &v("9.9.9")), Some(true));
        assert_eq!(satisfies("^abc", &v("1.0.0")), None);
    }
}
