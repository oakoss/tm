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
    fn a_symmetric_link_is_one_pair_either_way_round() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let relates = link_type("relates_to").unwrap();
        assert_eq!(canonical(relates, a, b), canonical(relates, b, a));
        let blocks = link_type(BLOCKS).unwrap();
        assert_eq!(canonical(blocks, b, a), (b, a));
    }
}
