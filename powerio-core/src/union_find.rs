//! Disjoint set forest over dense indices: the one union-find and component
//! labeling of the workspace. The transmission readers join topology nodes
//! into calculated buses and buses into islands with it, and the
//! distribution, problem, composition and matrix crates label their zones,
//! islands and cycles with it.

/// A disjoint set forest over the indices `0..len`.
#[derive(Debug, Clone)]
pub struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    /// Every index in its own set.
    #[must_use]
    pub fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    /// The representative of `value`'s set, compressing the visited chain.
    ///
    /// The walk is iterative. A source lists its switches in whatever order it
    /// likes, and joining each node to the one before it builds a chain as
    /// long as the node count, so recursion here would exhaust the stack on a
    /// large node breaker model.
    pub fn find(&mut self, value: usize) -> usize {
        let mut root = value;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut walk = value;
        while self.parent[walk] != root {
            let next = self.parent[walk];
            self.parent[walk] = root;
            walk = next;
        }
        root
    }

    /// Merge the sets containing `first` and `second`; the representative of
    /// `first`'s set stays the representative. Callers order their calculated
    /// buses by that representative, so the rule is part of the decoded
    /// result and not an implementation detail. Returns whether the two were
    /// in different sets, so a caller can detect a cycle.
    pub fn union(&mut self, first: usize, second: usize) -> bool {
        let first = self.find(first);
        let second = self.find(second);
        if first == second {
            return false;
        }
        self.parent[second] = first;
        true
    }

    /// Merge the sets containing `first` and `second`, keeping the smaller
    /// representative: the rule under which each set's representative is its
    /// smallest member. Returns whether the two were in different sets.
    pub fn union_keep_smaller(&mut self, first: usize, second: usize) -> bool {
        let first = self.find(first);
        let second = self.find(second);
        self.union(first.min(second), first.max(second))
    }

    /// Each index's representative: two indices share a label iff they are in
    /// one set, and a representative labels itself.
    #[must_use]
    pub fn into_labeling(mut self) -> Vec<usize> {
        (0..self.parent.len())
            .map(|value| self.find(value))
            .collect()
    }
}

/// Representative labels of the components `edges` join over `len` indices:
/// two indices share a label iff a path of edges joins them, and a
/// representative labels itself. The one component labeling behind every
/// island partition and island label of the workspace.
#[must_use]
pub fn calc_component_labels(
    len: usize,
    edges: impl IntoIterator<Item = (usize, usize)>,
) -> Vec<usize> {
    let mut sets = UnionFind::new(len);
    for (first, second) in edges {
        sets.union(first, second);
    }
    sets.into_labeling()
}

#[cfg(test)]
mod tests {
    use super::UnionFind;

    #[test]
    fn a_long_chain_of_joins_resolves_without_recursion() {
        const LEN: usize = 500_000;
        let mut union = UnionFind::new(LEN);
        // Joining each new root to the preceding node in increasing order
        // grows the parent chain by one on every call.
        for value in 0..LEN - 1 {
            union.union(value + 1, value);
        }
        let root = union.find(0);
        assert!((0..LEN).all(|value| union.find(value) == root));
    }

    #[test]
    fn the_smaller_representative_can_be_kept() {
        let mut union = UnionFind::new(4);
        assert!(union.union_keep_smaller(3, 1));
        assert_eq!(union.find(3), 1);
        assert!(!union.union_keep_smaller(1, 3));
    }

    #[test]
    fn the_first_set_keeps_its_representative() {
        let mut union = UnionFind::new(6);
        union.union(3, 1);
        assert_eq!(union.find(1), 3);
        union.union(2, 3);
        assert_eq!(union.find(1), 2);
        assert_ne!(union.find(4), union.find(5));
    }
}
