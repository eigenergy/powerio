//! Disjoint set forest over dense indices, used to join topology nodes into
//! calculated buses.

pub(crate) struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    pub(crate) fn new(len: usize) -> Self {
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
    pub(crate) fn find(&mut self, value: usize) -> usize {
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
    /// result and not an implementation detail.
    pub(crate) fn union(&mut self, first: usize, second: usize) {
        let first = self.find(first);
        let second = self.find(second);
        if first != second {
            self.parent[second] = first;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UnionFind;

    #[test]
    fn a_long_chain_of_joins_resolves_without_recursion() {
        const LEN: usize = 500_000;
        let mut union = UnionFind::new(LEN);
        // Joining each node to the one after it is the order that builds the
        // deepest chain under this merge rule.
        for value in (0..LEN - 1).rev() {
            union.union(value + 1, value);
        }
        let root = union.find(0);
        assert!((0..LEN).all(|value| union.find(value) == root));
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
