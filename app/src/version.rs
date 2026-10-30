//! Larrier's version: the release number (Semantic Versioning, from the
//! workspace's Cargo.toml), the commit it was built from and that commit's
//! date (baked in by build.rs), and every release's notes (CHANGELOG.md,
//! built in so they read offline).

use std::cmp::Ordering;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = env!("LARRIER_COMMIT");
pub const COMMIT_DATE: &str = env!("LARRIER_COMMIT_DATE");
const CHANGELOG: &str = include_str!("../../CHANGELOG.md");

/// "0.2.0 (a4a9ebc12, 2026-10-30)".
pub fn long() -> String {
    match (COMMIT, COMMIT_DATE) {
        ("" | "unknown", _) => VERSION.to_string(),
        (c, "") => format!("{VERSION} ({c})"),
        (c, d) => format!("{VERSION} ({c}, {d})"),
    }
}

/// A preview build (`0.3.0-beta.1`).
pub fn is_prerelease() -> bool {
    Version::parse(VERSION).map(|v| !v.pre.is_empty()).unwrap_or(false)
}

/// A semantic version: `1.2.3`, `1.2.3-beta.2` (a `v` prefix and `+build`
/// metadata are accepted and ignored).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, p.split('.').map(str::to_string).collect()),
            None => (s, Vec::new()),
        };
        let mut nums = core.split('.').map(|p| p.parse::<u64>().ok());
        let v = Version { major: nums.next()??, minor: nums.next().unwrap_or(Some(0))?, patch: nums.next().unwrap_or(Some(0))?, pre };
        nums.next().is_none().then_some(v)
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(o.major, o.minor, o.patch)).then_with(|| match (self.pre.is_empty(), o.pre.is_empty()) {
            // A release comes after its previews.
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&o.pre) {
                    let ord = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        _ => a.cmp(b),
                    };
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                self.pre.len().cmp(&o.pre.len())
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// `a` is a newer version than `b`.
pub fn newer(a: &str, b: &str) -> bool {
    match (Version::parse(a), Version::parse(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// A release's notes from the changelog (its section, without the heading).
pub fn notes(version: &str) -> Option<String> {
    let want = Version::parse(version)?;
    let mut out: Option<String> = None;
    for line in CHANGELOG.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            if out.is_some() {
                break;
            }
            // "## 0.2.0 — 2026-11-02"
            let v = h.split_whitespace().next().unwrap_or("");
            if Version::parse(v).as_ref() == Some(&want) {
                out = Some(String::new());
            }
            continue;
        }
        if let Some(o) = out.as_mut() {
            o.push_str(line);
            o.push('\n');
        }
    }
    out.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The release date the changelog gives a version.
pub fn released_on(version: &str) -> Option<String> {
    let want = Version::parse(version)?;
    CHANGELOG.lines().filter_map(|l| l.strip_prefix("## ")).find_map(|h| {
        let mut parts = h.split_whitespace();
        let v = parts.next()?;
        (Version::parse(v).as_ref() == Some(&want)).then(|| parts.filter(|p| *p != "—" && *p != "-").collect::<Vec<_>>().join(" "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_order() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("v1.0.0", "0.9.9"));
        assert!(newer("0.10.0", "0.9.0"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.0.9", "0.1.0"));
        // Previews come before their release, in order.
        assert!(newer("0.2.0", "0.2.0-beta.3"));
        assert!(!newer("0.2.0-beta.3", "0.2.0"));
        assert!(newer("0.2.0-beta.10", "0.2.0-beta.9"));
        assert!(newer("0.2.0-beta.1", "0.2.0-alpha.4"));
        assert!(newer("0.2.0-beta.1", "0.1.9"));
        assert!(newer("0.2.0-rc.1", "0.2.0-beta"));
        assert!(!newer("garbage", "0.1.0"));
        assert_eq!(Version::parse("1.2.3+build.5").map(|v| (v.major, v.minor, v.patch)), Some((1, 2, 3)));
        assert_eq!(Version::parse("1.2"), Some(Version { major: 1, minor: 2, patch: 0, pre: Vec::new() }));
        assert_eq!(Version::parse("1.2.3.4"), None);
    }

    #[test]
    fn this_version_has_notes() {
        assert!(Version::parse(VERSION).is_some());
        assert!(notes(VERSION).is_some(), "CHANGELOG.md has no section for {VERSION}");
        assert!(released_on(VERSION).is_some());
    }
}
