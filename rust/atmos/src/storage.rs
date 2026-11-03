//! Coordinate-indexed records. Snapshots share cold pages, not individual heap allocations.
use std::{
    ops::{Index, IndexMut},
    sync::Arc,
};

// Bound collateral gas-array clones when machinery writes to scattered coordinates.
const PAGE_SIZE: usize = 8;
type Page<T> = [Option<T>; PAGE_SIZE];

#[derive(Clone, Debug)]
pub struct Storage<T> {
    pages: Vec<Option<Arc<Page<T>>>>,
    len: usize,
}

impl<T> Default for Storage<T> {
    fn default() -> Self {
        Self {
            pages: Vec::new(),
            len: 0,
        }
    }
}

impl<T> Storage<T> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn get(&self, id: &u32) -> Option<&T> {
        let id = *id as usize;
        self.pages.get(id / PAGE_SIZE)?.as_ref()?[id % PAGE_SIZE].as_ref()
    }
    pub fn contains_key(&self, id: &u32) -> bool {
        self.get(id).is_some()
    }
    pub fn iter(&self) -> impl Iterator<Item = (u32, &T)> {
        self.pages
            .iter()
            .enumerate()
            .filter_map(|(index, page)| page.as_ref().map(|p| (index, p)))
            .flat_map(|(index, page)| {
                page.iter().enumerate().filter_map(move |(offset, value)| {
                    value
                        .as_ref()
                        .map(|v| ((index * PAGE_SIZE + offset) as u32, v))
                })
            })
    }
    pub fn keys(&self) -> impl Iterator<Item = u32> + '_ {
        self.iter().map(|(id, _)| id)
    }
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.iter().map(|(_, value)| value)
    }
}

impl<T: Clone> Storage<T> {
    pub fn get_mut(&mut self, id: &u32) -> Option<&mut T> {
        // Do not copy a shared page for a missing entry.
        self.get(id)?;
        let id = *id as usize;
        Arc::make_mut(self.pages[id / PAGE_SIZE].as_mut().unwrap())[id % PAGE_SIZE].as_mut()
    }
    pub fn insert(&mut self, id: u32, value: T) -> Option<T> {
        let id = id as usize;
        self.pages
            .resize_with(self.pages.len().max(id / PAGE_SIZE + 1), || None);
        let page = self.pages[id / PAGE_SIZE]
            .get_or_insert_with(|| Arc::new(std::array::from_fn(|_| None)));
        let previous = Arc::make_mut(page)[id % PAGE_SIZE].replace(value);
        self.len += usize::from(previous.is_none());
        previous
    }
    pub fn remove(&mut self, id: &u32) -> Option<T> {
        self.get(id)?;
        let id = *id as usize;
        let page = &mut self.pages[id / PAGE_SIZE];
        let result = Arc::make_mut(page.as_mut().unwrap())[id % PAGE_SIZE].take();
        self.len -= 1;
        if page.as_ref().unwrap().iter().all(Option::is_none) {
            *page = None;
        }
        result
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.pages
            .iter_mut()
            .filter_map(Option::as_mut)
            .flat_map(|page| Arc::make_mut(page).iter_mut().filter_map(Option::as_mut))
    }
    pub(crate) fn pair_mut(&mut self, a: u32, b: u32) -> (&mut T, &mut T) {
        assert_ne!(a, b);
        let (lo, hi) = (a.min(b) as usize, a.max(b) as usize);
        let (left, right) = if lo / PAGE_SIZE == hi / PAGE_SIZE {
            let page = Arc::make_mut(self.pages[lo / PAGE_SIZE].as_mut().unwrap());
            let (left, right) = page.split_at_mut(hi % PAGE_SIZE);
            (&mut left[lo % PAGE_SIZE], &mut right[0])
        } else {
            let (left, right) = self.pages.split_at_mut(hi / PAGE_SIZE);
            (
                &mut Arc::make_mut(left[lo / PAGE_SIZE].as_mut().unwrap())[lo % PAGE_SIZE],
                &mut Arc::make_mut(right[0].as_mut().unwrap())[hi % PAGE_SIZE],
            )
        };
        let pair = (left.as_mut().unwrap(), right.as_mut().unwrap());
        if a < b {
            pair
        } else {
            (pair.1, pair.0)
        }
    }
    /// The source is authoritative for the whole page. Share it once, even if several IDs changed.
    pub(crate) fn synchronize(&mut self, source: &Self, ids: impl Iterator<Item = u32>) {
        self.pages.resize_with(source.pages.len(), || None);
        let mut previous = None;
        for id in ids {
            let index = id as usize / PAGE_SIZE;
            if previous != Some(index) {
                self.pages[index].clone_from(&source.pages[index]);
                previous = Some(index);
            }
        }
        self.len = source.len;
    }
}

impl<T> Index<&u32> for Storage<T> {
    type Output = T;
    fn index(&self, id: &u32) -> &T {
        self.get(id).expect("Missing record")
    }
}
impl<T: Clone> IndexMut<&u32> for Storage<T> {
    fn index_mut(&mut self, id: &u32) -> &mut T {
        self.get_mut(id).expect("Missing record")
    }
}
impl<T: Clone> FromIterator<(u32, T)> for Storage<T> {
    fn from_iter<I: IntoIterator<Item = (u32, T)>>(iter: I) -> Self {
        let mut result = Self::new();
        for (id, value) in iter {
            result.insert(id, value);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_holes_replacement_and_zero_id() {
        let mut store: Storage<_> = [(300, 3), (0, 0), (129, 1), (128, 2)].into_iter().collect();
        assert_eq!(store.keys().collect::<Vec<_>>(), [0, 128, 129, 300]);
        assert_eq!(store.insert(128, 4), Some(2));
        assert_eq!(store.remove(&129), Some(1));
        assert_eq!(store.remove(&129), None);
        assert_eq!(store.get(&u32::MAX), None);
        assert_eq!(store.len(), 3);
        assert_eq!(store.values().copied().collect::<Vec<_>>(), [0, 4, 3]);
    }

    #[test]
    fn snapshots_survive_same_and_different_page_pair_writes() {
        let last = PAGE_SIZE as u32 - 1;
        let next = PAGE_SIZE as u32;
        let mut store: Storage<_> = [(0, 1), (last, 2), (next, 3)].into_iter().collect();
        let snapshot = store.clone();
        assert!(Arc::ptr_eq(
            store.pages[1].as_ref().unwrap(),
            snapshot.pages[1].as_ref().unwrap()
        ));
        let (a, b) = store.pair_mut(last, 0);
        *a = 20;
        *b = 10;
        assert!(Arc::ptr_eq(
            store.pages[1].as_ref().unwrap(),
            snapshot.pages[1].as_ref().unwrap()
        ));
        let (a, b) = store.pair_mut(0, next);
        *a += 1;
        *b = 30;
        assert_eq!(snapshot.values().copied().collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(store.values().copied().collect::<Vec<_>>(), [11, 20, 30]);
        store.remove(&next);
        assert!(store.pages[1].is_none());
        assert_eq!(snapshot[&next], 3);
    }

    #[test]
    fn synchronize_reshare_pages_with_insertions_and_removals() {
        let next = PAGE_SIZE as u32;
        let mut current: Storage<_> = [(0, 1), (next - 1, 2), (next, 3)].into_iter().collect();
        let mut spare = current.clone();
        current.remove(&next);
        current.insert(2 * next - 1, 4);
        current.insert(4 * next, 5);
        spare.synchronize(&current, [next, 2 * next - 1, 4 * next].into_iter());
        assert_eq!(
            spare.iter().collect::<Vec<_>>(),
            current.iter().collect::<Vec<_>>()
        );
        assert_eq!(spare.len(), current.len());
        assert!(Arc::ptr_eq(
            spare.pages[1].as_ref().unwrap(),
            current.pages[1].as_ref().unwrap()
        ));
        let snapshot = current.clone();
        spare.values_mut().for_each(|v| *v += 10);
        assert_eq!(
            current.iter().collect::<Vec<_>>(),
            snapshot.iter().collect::<Vec<_>>()
        );
    }
}
