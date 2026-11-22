//! Release version ordering: `1.0.0-rc.99 < 1.0.0-rc.100 < 1.0.0 < 1.0.1-rc.1`.

use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Ident {
    Num(u64),
    Alpha(String),
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Ident::Num(a), Ident::Num(b)) => a.cmp(b),
            (Ident::Num(_), Ident::Alpha(_)) => Ordering::Less,
            (Ident::Alpha(_), Ident::Num(_)) => Ordering::Greater,
            (Ident::Alpha(a), Ident::Alpha(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A parsed `MAJOR.MINOR.PATCH[-pre.release]` version. A leading `v` is allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    core: [u64; 3],
    pre: Vec<Ident>,
}

impl Version {
    pub fn parse(text: &str) -> Result<Version, String> {
        let bad = || format!("'{text}' is not a MAJOR.MINOR.PATCH[-pre] version");
        let s = text.strip_prefix('v').unwrap_or(text);
        let s = s.split('+').next().unwrap_or(s);
        let (core_s, pre_s) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let parts: Vec<&str> = core_s.split('.').collect();
        if parts.len() != 3 {
            return Err(bad());
        }
        let mut core = [0u64; 3];
        for (slot, part) in core.iter_mut().zip(&parts) {
            *slot = part.parse().map_err(|_| bad())?;
        }
        let mut pre = Vec::new();
        if let Some(p) = pre_s {
            for id in p.split('.') {
                if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    return Err(bad());
                }
                pre.push(match id.parse::<u64>() {
                    Ok(n) => Ident::Num(n),
                    Err(_) => Ident::Alpha(id.to_string()),
                });
            }
        }
        Ok(Version { core, pre })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A release tag as it may appear in a URL path: `v<MAJOR>.<MINOR>.<PATCH>` with an
/// optional `-pre.release` part, nothing else (no `+build`, no slashes, no spaces).
pub fn valid_tag(tag: &str) -> bool {
    tag.starts_with('v')
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && Version::parse(tag).is_ok()
}

/// `<owner>/<repo>`: one slash, each side GitHub-name characters, no dot-only parts.
pub fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let ok = |p: &str| {
        !p.is_empty()
            && p != "."
            && p != ".."
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    };
    ok(owner) && ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn orders_release_candidates_numerically() {
        assert!(v("1.0.0-rc.99") < v("1.0.0-rc.100"));
        assert!(v("v1.0.0-rc.100") > v("1.0.0-rc.9"));
    }

    #[test]
    fn a_release_outranks_its_candidates() {
        assert!(v("1.0.0-rc.100") < v("1.0.0"));
        assert!(v("1.0.0") < v("1.0.1-rc.1"));
        assert!(v("1.0.1-rc.1") < v("1.0.1"));
    }

    #[test]
    fn core_parts_compare_as_numbers() {
        assert!(v("1.9.0") < v("1.10.0"));
        assert!(v("2.0.0") > v("1.99.99"));
    }

    #[test]
    fn equal_and_build_metadata() {
        assert_eq!(v("v1.2.3"), v("1.2.3+build5"));
        assert_eq!(v("1.0.0-rc.1").cmp(&v("1.0.0-rc.1")), Ordering::Equal);
    }

    #[test]
    fn rejects_non_versions() {
        for bad in [
            "",
            "main",
            "1.2",
            "1.2.x",
            "1.2.3-",
            "1.2.3-rc..1",
            "v1.2.3.4",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn tags_are_strict_before_they_reach_a_url() {
        for good in ["v1.2.3", "v1.0.0-rc.100"] {
            assert!(valid_tag(good), "{good}");
        }
        for bad in [
            "1.2.3",
            "v1.2.3+build",
            "v1.2.3/../x",
            "v1.2.3 ",
            "v1.2.3?x=1",
            "v1.2.3#f",
            "v1.2",
            "main",
        ] {
            assert!(!valid_tag(bad), "{bad}");
        }
    }

    #[test]
    fn repos_are_owner_slash_name() {
        assert!(valid_repo("unsigned-gg/xerj"));
        assert!(valid_repo("a.b/c_d-e"));
        for bad in [
            "", "xerj", "a/b/c", "/b", "a/", "../x", "a/..", "a b/c", "a/b?x", "a/b#f", "a:b/c",
        ] {
            assert!(!valid_repo(bad), "{bad}");
        }
    }
}
