use swirvo_core::FormatId;

/// 256-bit set of formats: four words, so intersections are four ANDs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FormatSet([u64; 4]);

impl FormatSet {
    pub fn insert(&mut self, f: FormatId) {
        let i = f.index();
        self.0[i >> 6] |= 1 << (i & 63);
    }
    pub fn remove(&mut self, f: FormatId) {
        let i = f.index();
        self.0[i >> 6] &= !(1 << (i & 63));
    }
    pub fn contains(&self, f: FormatId) -> bool {
        let i = f.index();
        self.0[i >> 6] & (1 << (i & 63)) != 0
    }
    pub fn and(self, o: FormatSet) -> FormatSet {
        FormatSet([
            self.0[0] & o.0[0],
            self.0[1] & o.0[1],
            self.0[2] & o.0[2],
            self.0[3] & o.0[3],
        ])
    }
    pub fn or(self, o: FormatSet) -> FormatSet {
        FormatSet([
            self.0[0] | o.0[0],
            self.0[1] | o.0[1],
            self.0[2] | o.0[2],
            self.0[3] | o.0[3],
        ])
    }
    pub fn len(&self) -> usize {
        self.0.iter().map(|w| w.count_ones() as usize).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.0 == [0; 4]
    }
    /// Ascending ids, using trailing-zero counts to skip empty runs.
    pub fn iter(&self) -> impl Iterator<Item = FormatId> + '_ {
        self.0.iter().enumerate().flat_map(|(w, &word)| {
            let mut bits = word;
            std::iter::from_fn(move || {
                if bits == 0 {
                    return None;
                }
                let t = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                Some(FormatId((w * 64 + t) as u16))
            })
        })
    }
}

impl FromIterator<FormatId> for FormatSet {
    fn from_iter<I: IntoIterator<Item = FormatId>>(iter: I) -> Self {
        let mut s = FormatSet::default();
        for f in iter {
            s.insert(f);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    proptest! {
        #[test]
        fn matches_btreeset(a in proptest::collection::btree_set(0u16..256, 0..60),
                            b in proptest::collection::btree_set(0u16..256, 0..60)) {
            let sa: FormatSet = a.iter().map(|&i| FormatId(i)).collect();
            let sb: FormatSet = b.iter().map(|&i| FormatId(i)).collect();
            let inter: BTreeSet<u16> = a.intersection(&b).copied().collect();
            let got: BTreeSet<u16> = sa.and(sb).iter().map(|f| f.0).collect();
            prop_assert_eq!(&got, &inter);
            prop_assert_eq!(sa.and(sb).len(), inter.len());
            prop_assert_eq!(sa.or(sb).len(), a.union(&b).count());
        }
    }
}
