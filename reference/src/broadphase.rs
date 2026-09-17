//! Deterministic broad phase (RFC-0008 §7 interchangeable backends under one
//! semantic contract).
//!
//! Two interchangeable backends implement the same [`BroadPhase::pairs`]
//! contract:
//!
//! * [`UniformGrid`]: every AABB is inserted into each cell it overlaps;
//!   candidate pairs are the unique id-pairs sharing a cell, emitted in sorted
//!   order so results are bit-stable regardless of cell hashing.
//! * [`Bvh`]: a deterministic median-split bounding-volume hierarchy over
//!   id-sorted AABBs; candidate pairs are the unique id-pairs of overlapping
//!   sibling nodes, emitted in sorted order.
//!
//! Both are deterministic and interchangeable — for the same input they produce
//! the exact same pair set (proven against a shared brute-force oracle).

use crate::math::{Aabb, Vec3};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Cell(i64, i64, i64);

/// The semantic broad-phase contract both backends implement.
pub trait BroadPhase {
    /// Unique unordered candidate pairs `(min_id, max_id)`, sorted.
    fn pairs(&self) -> Vec<(u128, u128)>;
}

/// A deterministic broad-phase accelerator.
#[derive(Clone, Debug, Default)]
pub struct UniformGrid {
    cell_size: f64,
    buckets: std::collections::BTreeMap<Cell, Vec<u128>>,
}

impl UniformGrid {
    /// Builds a grid over `bounds`. Cell size is the largest half-extent times
    /// two (at least a small epsilon) so big bodies do not explode cell count.
    pub fn build(bounds: &[(u128, Aabb)]) -> Self {
        let mut max_extent = 1.0f64;
        for (_, b) in bounds {
            let half = b.half_extents();
            max_extent = max_extent.max(half.x).max(half.y).max(half.z);
        }
        let cell_size = max_extent * 2.0;
        let mut buckets: std::collections::BTreeMap<Cell, Vec<u128>> =
            std::collections::BTreeMap::new();
        for (id, aabb) in bounds {
            let lo = cell_of(aabb.min, cell_size);
            let hi = cell_of(aabb.max, cell_size);
            for cx in lo.0..=hi.0 {
                for cy in lo.1..=hi.1 {
                    for cz in lo.2..=hi.2 {
                        buckets.entry(Cell(cx, cy, cz)).or_default().push(*id);
                    }
                }
            }
        }
        Self { cell_size, buckets }
    }

    /// Unique unordered candidate pairs `(min_id, max_id)`, sorted.
    pub fn pairs(&self) -> Vec<(u128, u128)> {
        let mut set: BTreeSet<(u128, u128)> = BTreeSet::new();
        for ids in self.buckets.values() {
            for i in 0..ids.len() {
                for j in (i + 1)..ids.len() {
                    let (a, b) = if ids[i] < ids[j] {
                        (ids[i], ids[j])
                    } else {
                        (ids[j], ids[i])
                    };
                    set.insert((a, b));
                }
            }
        }
        set.into_iter().collect()
    }

    pub fn cell_size(&self) -> f64 {
        self.cell_size
    }
}

impl BroadPhase for UniformGrid {
    fn pairs(&self) -> Vec<(u128, u128)> {
        self.pairs()
    }
}

fn cell_of(p: Vec3, cell: f64) -> Cell {
    Cell(
        (p.x / cell).floor() as i64,
        (p.y / cell).floor() as i64,
        (p.z / cell).floor() as i64,
    )
}

/// A deterministic bounding-volume hierarchy broad phase.
///
/// The input AABBs are sorted by id, then recursively median-split along the
/// largest center-extent axis down to a leaf cap. Each node stores the union
/// AABB of its subtree. Because leaves are id-sorted and each internal node's
/// children are a contiguous, non-overlapping partition, the hierarchy is
/// unique for a given input — no hashing, no instability. `pairs()` descends
/// only into nodes whose AABBs intersect and emits the unique `(min,max)` pairs
/// of overlapping leaves, in sorted order.
#[derive(Clone, Debug, Default)]
pub struct Bvh {
    nodes: Vec<BvhNode>,
    /// Id of the first leaf encountered, by depth-first construction order.
    root: usize,
}

#[derive(Clone, Debug, Default)]
struct BvhNode {
    aabb: Option<Aabb>,
    /// The items held by this node; non-empty only for a leaf.
    items: Vec<(u128, Aabb)>,
    /// Child indices (`usize::MAX` for a leaf).
    left: usize,
    right: usize,
}

/// The leaf-cap threshold controlling hierarchy depth.
pub const LEAF_CAP: usize = 4;

impl Bvh {
    /// Builds a median-split BVH over `bounds` (ids sorted for stability).
    pub fn build(bounds: &[(u128, Aabb)]) -> Self {
        if bounds.is_empty() {
            return Self {
                nodes: Vec::new(),
                root: usize::MAX,
            };
        }
        let mut items: Vec<(u128, Aabb)> = bounds.to_vec();
        items.sort_by_key(|(id, _)| *id);
        let mut nodes = Vec::new();
        let root = build_node(&mut items, &mut nodes);
        Self { nodes, root }
    }

    /// Unique unordered candidate pairs `(min_id, max_id)`, sorted.
    pub fn pairs(&self) -> Vec<(u128, u128)> {
        let mut set: BTreeSet<(u128, u128)> = BTreeSet::new();
        if self.nodes.is_empty() {
            return Vec::new();
        }
        self.collect_pairs(self.root, &mut set);
        set.into_iter().collect()
    }

    fn children(&self, index: usize) -> Option<(usize, usize)> {
        if !self.nodes[index].items.is_empty() {
            return None;
        }
        Some((self.nodes[index].left, self.nodes[index].right))
    }

    fn collect_pairs(&self, index: usize, out: &mut BTreeSet<(u128, u128)>) {
        // A leaf contributes every overlapping pair among its own items.
        if let Some(items) = self.leaf_items(index) {
            for i in 0..items.len() {
                for j in (i + 1)..items.len() {
                    if items[i].1.overlaps(items[j].1) {
                        let (ia, ib) = (items[i].0, items[j].0);
                        out.insert((ia.min(ib), ia.max(ib)));
                    }
                }
            }
            return;
        }
        let (a, b) = self.children(index).unwrap();
        self.pairs_between(a, b, out);
        self.collect_pairs(a, out);
        self.collect_pairs(b, out);
    }

    fn leaf_items(&self, index: usize) -> Option<&[(u128, Aabb)]> {
        if self.nodes[index].items.is_empty() {
            None
        } else {
            Some(&self.nodes[index].items)
        }
    }

    /// Emits every overlapping pair with one item under `a` and one under `b`,
    /// descending only into node pairs whose AABBs intersect.
    fn pairs_between(&self, a: usize, b: usize, out: &mut BTreeSet<(u128, u128)>) {
        let (na, nb) = (&self.nodes[a], &self.nodes[b]);
        let (Some(aa), Some(bb)) = (na.aabb, nb.aabb) else {
            return;
        };
        if !aa.overlaps(bb) {
            return;
        }
        match (self.leaf_items(a), self.leaf_items(b)) {
            (Some(ia), Some(ib)) => {
                for (id_a, box_a) in ia {
                    for (id_b, box_b) in ib {
                        if box_a.overlaps(*box_b) {
                            let (x, y) = (id_a.min(id_b), id_a.max(id_b));
                            out.insert((*x, *y));
                        }
                    }
                }
            }
            (Some(_), None) => {
                let (ca, cb) = self.children(b).unwrap();
                self.pairs_between(a, ca, out);
                self.pairs_between(a, cb, out);
            }
            (None, Some(_)) => {
                let (ca, cb) = self.children(a).unwrap();
                self.pairs_between(ca, b, out);
                self.pairs_between(cb, b, out);
            }
            (None, None) => {
                let (aa1, aa2) = self.children(a).unwrap();
                let (bb1, bb2) = self.children(b).unwrap();
                self.pairs_between(aa1, bb1, out);
                self.pairs_between(aa1, bb2, out);
                self.pairs_between(aa2, bb1, out);
                self.pairs_between(aa2, bb2, out);
            }
        }
    }
}

/// Recursively builds a median-split subtree over `items`, appending nodes in
/// post-order (children before parent) and returning the root node index.
fn build_node(items: &mut [(u128, Aabb)], nodes: &mut Vec<BvhNode>) -> usize {
    debug_assert!(!items.is_empty());
    let mut union = items[0].1;
    for (_, aabb) in items.iter().skip(1) {
        union = union.union(*aabb);
    }
    if items.len() <= LEAF_CAP {
        let index = nodes.len();
        nodes.push(BvhNode {
            aabb: Some(union),
            items: items.to_vec(),
            left: usize::MAX,
            right: usize::MAX,
        });
        return index;
    }
    // Split along the largest center-extent axis at the median, id-stable.
    let axis = largest_axis(&union);
    items.sort_by(|(ia, aa), (ib, bb)| {
        center(axis, aa)
            .partial_cmp(&center(axis, bb))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ia.cmp(ib))
    });
    let mid = items.len() / 2;
    let (left, right) = items.split_at_mut(mid);
    let left_index = build_node(left, nodes);
    let right_index = build_node(right, nodes);
    let index = nodes.len();
    nodes.push(BvhNode {
        aabb: Some(union),
        items: Vec::new(),
        left: left_index,
        right: right_index,
    });
    index
}

fn largest_axis(aabb: &Aabb) -> usize {
    let d = aabb.half_extents();
    if d.x >= d.y && d.x >= d.z {
        0
    } else if d.y >= d.z {
        1
    } else {
        2
    }
}

fn center(axis: usize, aabb: &Aabb) -> f64 {
    match axis {
        0 => (aabb.min.x + aabb.max.x) * 0.5,
        1 => (aabb.min.y + aabb.max.y) * 0.5,
        _ => (aabb.min.z + aabb.max.z) * 0.5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_at(id: u128, x: f64) -> (u128, Aabb) {
        (
            id,
            Aabb::from_center_half(Vec3::new(x, 0.0, 0.0), Vec3::new(0.5, 0.5, 0.5)),
        )
    }

    #[test]
    fn broadphase_pairs_are_exactly_overlaps() {
        let bounds = vec![
            box_at(1, 0.0), // overlaps 2 at x=0.4
            box_at(2, 0.4),
            box_at(3, 10.0), // far away
            box_at(4, 0.6),  // overlaps 1 and 2
        ];
        let grid = UniformGrid::build(&bounds);
        let pairs = grid.pairs();
        assert_eq!(pairs, vec![(1, 2), (1, 4), (2, 4)]);
    }

    #[test]
    fn broadphase_is_deterministic_for_any_insertion_order() {
        let a = vec![box_at(3, 10.0), box_at(1, 0.0), box_at(2, 0.4)];
        let b = vec![box_at(1, 0.0), box_at(2, 0.4), box_at(3, 10.0)];
        assert_eq!(
            UniformGrid::build(&a).pairs(),
            UniformGrid::build(&b).pairs()
        );
    }

    fn box_2d(id: u128, x: f64, y: f64) -> (u128, Aabb) {
        (
            id,
            Aabb::from_center_half(Vec3::new(x, y, 0.0), Vec3::new(0.3, 0.3, 0.3)),
        )
    }

    fn brute_force(bounds: &[(u128, Aabb)]) -> Vec<(u128, u128)> {
        let mut set: BTreeSet<(u128, u128)> = BTreeSet::new();
        for i in 0..bounds.len() {
            for j in (i + 1)..bounds.len() {
                let (ia, aa) = bounds[i];
                let (ib, bb) = bounds[j];
                if aa.overlaps(bb) {
                    let (x, y) = (ia.min(ib), ia.max(ib));
                    set.insert((x, y));
                }
            }
        }
        set.into_iter().collect()
    }

    /// Deterministic pseudo-random generator (no external dep).
    fn lcg(state: &mut u64) -> f64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((*state >> 11) as f64 / (1u64 << 53) as f64) * 20.0 - 10.0
    }

    #[test]
    fn bvh_matches_brute_force_and_grid_has_no_false_negatives() {
        let mut state: u64 = 0x1234_5678_9abc_def0;
        let mut bounds = Vec::new();
        for id in 1..=40u128 {
            bounds.push(box_2d(id, lcg(&mut state), lcg(&mut state)));
        }
        let expected = brute_force(&bounds);
        // The BVH with exact leaf AABBs returns exactly the true overlaps.
        assert_eq!(
            Bvh::build(&bounds).pairs(),
            expected,
            "BVH must equal the brute-force overlap set"
        );
        // The grid is a conservative broad phase: it may emit extra candidate
        // pairs, but it must never miss a true overlap (no false negatives).
        let grid = UniformGrid::build(&bounds).pairs();
        let grid: std::collections::BTreeSet<(u128, u128)> = grid.into_iter().collect();
        for pair in &expected {
            assert!(grid.contains(pair), "grid must not miss {pair:?}");
        }
    }

    #[test]
    fn bvh_is_deterministic_for_any_insertion_order() {
        let mut state: u64 = 0xdead_beef_cafe_f00d;
        let mut bounds = Vec::new();
        for id in 1..=30u128 {
            bounds.push(box_2d(id, lcg(&mut state), lcg(&mut state)));
        }
        let mut shuffled = bounds.clone();
        shuffled.reverse();
        assert_eq!(
            Bvh::build(&bounds).pairs(),
            Bvh::build(&shuffled).pairs(),
            "BVH result must be independent of insertion order"
        );
    }

    #[test]
    fn bvh_empty_and_single() {
        assert_eq!(Bvh::build(&[]).pairs(), Vec::<(u128, u128)>::new());
        let one = vec![box_at(1, 0.0)];
        assert_eq!(Bvh::build(&one).pairs(), Vec::<(u128, u128)>::new());
    }
}
