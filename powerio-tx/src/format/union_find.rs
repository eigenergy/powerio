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
    pub(crate) fn find(&mut self, value: usize) -> usize {
        let parent = self.parent[value];
        if parent == value {
            value
        } else {
            let root = self.find(parent);
            self.parent[value] = root;
            root
        }
    }

    /// Merge the sets containing `first` and `second`; the representative of
    /// `first`'s set stays the representative.
    pub(crate) fn union(&mut self, first: usize, second: usize) {
        let first = self.find(first);
        let second = self.find(second);
        if first != second {
            self.parent[second] = first;
        }
    }
}
