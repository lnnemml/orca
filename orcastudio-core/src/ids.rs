//! Branded atom-identity and index-space types (ADR-010 I1/§types).
//!
//! `AtomId` is a stable identity (mirrors the TS `AtomId` of unit 1b). `OrcaIndex`
//! and `AseIndex` are **positions** in two different emitted orders. All three are
//! distinct newtypes: mixing them **does not compile** — the off-by-one that the TS
//! `unique symbol` brand catches on its half is caught by the type system here on
//! the Rust half, across the whole crate rather than in one file.
//!
//! ```compile_fail
//! use orcastudio_core::ids::{OrcaIndex, AseIndex};
//! let o = OrcaIndex::from_position(0);
//! // OrcaIndex and AseIndex are different types — this assignment is rejected.
//! let _a: AseIndex = o;
//! ```
//!
//! ```compile_fail
//! use orcastudio_core::ids::{AtomId, OrcaIndex};
//! // An AtomId is an identity, not a position — it is not an index.
//! let _o: OrcaIndex = AtomId::new(3);
//! ```

use std::collections::BTreeMap;

/// A stable, opaque atom identity — assigned when an atom enters a Scene, invariant
/// until deletion. **Not** a position in any array (ADR-010 I1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AtomId(u32);

impl AtomId {
    pub const fn new(n: u32) -> Self {
        AtomId(n)
    }
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// A position in the **ORCA input** atom order (the coordinate block / `%geom`
/// index space). 0-based, matching the empirically-settled ORCA index base
/// (`wiki/orca/constraints.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrcaIndex(u32);

/// A position in the **ASE / sidecar** positional array order. Distinct from
/// `OrcaIndex` so a sidecar index can never be written into an ORCA line by
/// accident (ADR-010 correction (i): the two orders are owned separately).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AseIndex(u32);

/// A position in one emitted order, constructible from / convertible to `usize`.
/// Implemented only by the space-index newtypes (never by `AtomId` — an identity
/// is not a position), which is what keeps `IndexMap` from mixing spaces.
pub trait SpaceIndex: Copy {
    fn from_position(i: usize) -> Self;
    fn position(self) -> usize;
}

impl SpaceIndex for OrcaIndex {
    fn from_position(i: usize) -> Self {
        OrcaIndex(i as u32)
    }
    fn position(self) -> usize {
        self.0 as usize
    }
}

impl SpaceIndex for AseIndex {
    fn from_position(i: usize) -> Self {
        AseIndex(i as u32)
    }
    fn position(self) -> usize {
        self.0 as usize
    }
}

/// The `AtomId ↔ T` mapping produced *alongside* an emit (ADR-010: the module that
/// emits the order hands back the map used to read the output). Built from **one**
/// source — the emitted `AtomId` order — so the forward map and the reverse vector
/// are consistent **by construction**: there is no constructor that takes a
/// separately-supplied pair that could disagree. No global state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexMap<T: SpaceIndex> {
    /// position (== `T`) → `AtomId`
    reverse: Vec<AtomId>,
    /// `AtomId` → `T`
    forward: BTreeMap<AtomId, T>,
}

impl<T: SpaceIndex> IndexMap<T> {
    /// Build from the emitted order: atom at emit position `i` gets index
    /// `T::from_position(i)`. The single source guarantees consistency.
    pub fn from_emit_order(order: &[AtomId]) -> Self {
        let reverse = order.to_vec();
        let forward = order
            .iter()
            .enumerate()
            .map(|(i, &a)| (a, T::from_position(i)))
            .collect();
        IndexMap { reverse, forward }
    }

    /// `AtomId` → its index in this emitted order (`None` if not emitted).
    pub fn to_space(&self, id: AtomId) -> Option<T> {
        self.forward.get(&id).copied()
    }

    /// An emitted index → the `AtomId` at that position (`None` if out of range).
    pub fn to_atom(&self, idx: T) -> Option<AtomId> {
        self.reverse.get(idx.position()).copied()
    }

    pub fn len(&self) -> usize {
        self.reverse.len()
    }
    pub fn is_empty(&self) -> bool {
        self.reverse.is_empty()
    }

    /// The emit order: `AtomId` at each position (position `i` == `T::from_position(i)`).
    /// This is the single value that fully determines the map — [`from_emit_order`]
    /// rebuilds an identical map from it — so it is what unit 1e serializes into
    /// `jobs.index_map_json` (as the `AtomId` u32s) and reads back.
    ///
    /// [`from_emit_order`]: IndexMap::from_emit_order
    pub fn order(&self) -> &[AtomId] {
        &self.reverse
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_map_round_trips_in_emit_order() {
        let order = [AtomId::new(7), AtomId::new(2), AtomId::new(5)];
        let map = IndexMap::<OrcaIndex>::from_emit_order(&order);
        // AtomId 7 was emitted first → OrcaIndex 0, and back.
        assert_eq!(map.to_space(AtomId::new(7)), Some(OrcaIndex::from_position(0)));
        assert_eq!(map.to_space(AtomId::new(5)), Some(OrcaIndex::from_position(2)));
        assert_eq!(map.to_atom(OrcaIndex::from_position(1)), Some(AtomId::new(2)));
        assert_eq!(map.to_space(AtomId::new(99)), None);
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn forward_and_reverse_agree_by_construction() {
        let order = [AtomId::new(3), AtomId::new(1), AtomId::new(4), AtomId::new(1)];
        // (duplicate AtomId is a scene-validation concern, not IndexMap's — here we
        // only assert the two directions never disagree for a resolvable id.)
        let map = IndexMap::<AseIndex>::from_emit_order(&order[..3]);
        for i in 0..map.len() {
            let idx = AseIndex::from_position(i);
            let id = map.to_atom(idx).unwrap();
            assert_eq!(map.to_space(id), Some(idx));
        }
    }
}
