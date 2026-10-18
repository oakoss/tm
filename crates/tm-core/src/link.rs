//! Links: typed relationships between two Issues, keyed canonically so the
//! same Link written from either end, on any Replica, is one element (ADR 0009).

use uuid::Uuid;

/// The field a Link's element is filed under; no other kind may write it.
pub const FIELD: &str = "links";

/// A built-in Link type; directional ones are stored outward only. Only
/// [`BUILT_IN`] constructs one, and [`canonical`] trusts its `symmetric`, so
/// pass the entry [`link_type`] returns rather than a modified copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct LinkType {
    pub id: &'static str,
    pub outward: &'static str,
    pub inward: &'static str,
    pub symmetric: bool,
}

pub const BLOCKS: &str = "blocks";

pub const BUILT_IN: [LinkType; 6] = [
    LinkType {
        id: BLOCKS,
        outward: "blocks",
        inward: "is blocked by",
        symmetric: false,
    },
    LinkType {
        id: "relates_to",
        outward: "relates to",
        inward: "relates to",
        symmetric: true,
    },
    LinkType {
        id: "duplicates",
        outward: "duplicates",
        inward: "is duplicated by",
        symmetric: false,
    },
    LinkType {
        id: "derived_from",
        outward: "is derived from",
        inward: "has derived",
        symmetric: false,
    },
    LinkType {
        id: "causes",
        outward: "causes",
        inward: "is caused by",
        symmetric: false,
    },
    LinkType {
        id: "supersedes",
        outward: "supersedes",
        inward: "is superseded by",
        symmetric: false,
    },
];

#[must_use]
pub fn link_type(id: &str) -> Option<&'static LinkType> {
    BUILT_IN.iter().find(|link_type| link_type.id == id)
}

/// The type a name means, and whether it reads from the target's end: its id,
/// outward name or inward name, without case and with `_` or `-` read as a
/// space, so `blocks`, `is blocked by` and `is_blocked_by` all resolve.
#[must_use]
pub fn by_name(name: &str) -> Option<(&'static LinkType, bool)> {
    let wanted = spoken(name);
    BUILT_IN.iter().find_map(|link_type| {
        if spoken(link_type.id) == wanted || spoken(link_type.outward) == wanted {
            Some((link_type, false))
        } else if spoken(link_type.inward) == wanted {
            Some((link_type, true))
        } else {
            None
        }
    })
}

fn spoken(name: &str) -> String {
    name.split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every outward and inward name, for an error to list.
#[must_use]
pub fn names() -> Vec<&'static str> {
    let mut names = Vec::new();
    for link_type in &BUILT_IN {
        names.push(link_type.outward);
        if link_type.inward != link_type.outward {
            names.push(link_type.inward);
        }
    }
    names
}

/// The `(source, target)` a Link between `from` and `to` is stored as: as
/// given for a directional type, ordered by bytes for a symmetric one.
#[must_use]
pub fn canonical(link_type: &LinkType, from: Uuid, to: Uuid) -> (Uuid, Uuid) {
    if link_type.symmetric && to < from {
        (to, from)
    } else {
        (from, to)
    }
}

/// The set element a Link occupies under its source Issue's `links` field.
/// Distinct for every valid type, since a type id cannot hold `/`.
#[must_use]
pub fn elem(link_type: &str, target: Uuid) -> String {
    format!("{link_type}/{target}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_ids_are_scheme_ids_and_unique() {
        for (i, built_in) in BUILT_IN.iter().enumerate() {
            assert!(crate::scheme::is_id(built_in.id), "{}", built_in.id);
            assert_eq!(link_type(built_in.id), Some(&BUILT_IN[i]));
        }
    }

    #[test]
    fn a_name_resolves_to_its_type_and_which_end_it_reads_from() {
        let blocks = link_type(BLOCKS).unwrap();
        for name in ["blocks", "BLOCKS", " blocks "] {
            assert_eq!(by_name(name), Some((blocks, false)), "{name}");
        }
        for name in ["is blocked by", "is_blocked_by", "Is-Blocked  By"] {
            assert_eq!(by_name(name), Some((blocks, true)), "{name}");
        }
        let relates = link_type("relates_to").unwrap();
        assert_eq!(by_name("relates to"), Some((relates, false)));
        assert_eq!(by_name("relates_to"), Some((relates, false)));
        let derived = link_type("derived_from").unwrap();
        assert_eq!(by_name("has derived"), Some((derived, true)));
        assert_eq!(by_name("is derived from"), Some((derived, false)));
        assert_eq!(by_name("derived_from"), Some((derived, false)));
        assert_eq!(by_name("blocked"), None);
        assert_eq!(by_name(""), None);
        assert!(names().contains(&"is blocked by"));
        assert_eq!(
            names().iter().filter(|name| **name == "relates to").count(),
            1
        );
    }

    #[test]
    fn a_symmetric_link_is_one_pair_either_way_round() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let relates = link_type("relates_to").unwrap();
        assert_eq!(canonical(relates, a, b), canonical(relates, b, a));
        let blocks = link_type(BLOCKS).unwrap();
        assert_eq!(canonical(blocks, b, a), (b, a));
    }
}
