use crate::histogram::lcs::find_lcs;
use crate::histogram::list_pool::{ListHandle, ListPool};
use crate::intern::Token;

/// Maximum number of occurrences tracked for a single token.
const MAX_CHAIN_LEN: u32 = 63;

struct Histogram {
    token_occurrences: Vec<ListHandle>,
    pool: ListPool,
}

pub(super) fn diff(
    before: &[Token],
    after: &[Token],
    removed: &mut [bool],
    added: &mut [bool],
    num_tokens: u32,
) {
    Histogram::new(num_tokens).run(before, after, removed, added);
}

impl Histogram {
    fn new(num_tokens: u32) -> Self {
        Self {
            token_occurrences: vec![ListHandle::default(); num_tokens as usize],
            pool: ListPool::new(2 * num_tokens),
        }
    }

    fn clear(&mut self) {
        self.pool.clear();
    }

    fn occurrences(&self, token: Token) -> &[u32] {
        self.token_occurrences[token.0 as usize].as_slice(&self.pool)
    }

    fn occurrence_count(&self, token: Token) -> u32 {
        self.token_occurrences[token.0 as usize].len(&self.pool)
    }

    fn populate(&mut self, before: &[Token]) {
        for (index, &token) in before.iter().enumerate() {
            self.token_occurrences[token.0 as usize].push(index as u32, &mut self.pool);
        }
    }

    fn run(
        &mut self,
        mut before: &[Token],
        mut after: &[Token],
        mut removed: &mut [bool],
        mut added: &mut [bool],
    ) {
        loop {
            if before.is_empty() {
                added.fill(true);
                return;
            }
            if after.is_empty() {
                removed.fill(true);
                return;
            }

            self.populate(before);
            match find_lcs(before, after, self) {
                Some(lcs) if lcs.len == 0 => {
                    removed.fill(true);
                    added.fill(true);
                    return;
                }
                Some(lcs) => {
                    self.run(
                        &before[..lcs.before_start as usize],
                        &after[..lcs.after_start as usize],
                        &mut removed[..lcs.before_start as usize],
                        &mut added[..lcs.after_start as usize],
                    );

                    let before_end = lcs.before_start + lcs.len;
                    before = &before[before_end as usize..];
                    removed = &mut removed[before_end as usize..];

                    let after_end = lcs.after_start + lcs.len;
                    after = &after[after_end as usize..];
                    added = &mut added[after_end as usize..];
                }
                None => {
                    // Highly repetitive inputs exceed the histogram chain
                    // limit. Marking the region changed keeps this crate
                    // histogram-only while remaining conservative for callers.
                    removed.fill(true);
                    added.fill(true);
                    return;
                }
            }
        }
    }
}

mod lcs {
    use super::{Histogram, MAX_CHAIN_LEN};
    use crate::intern::Token;

    #[derive(Default)]
    pub(super) struct Lcs {
        pub(super) before_start: u32,
        pub(super) after_start: u32,
        pub(super) len: u32,
    }

    pub(super) fn find_lcs(
        before: &[Token],
        after: &[Token],
        histogram: &mut Histogram,
    ) -> Option<Lcs> {
        let mut search = LcsSearch {
            lcs: Lcs::default(),
            min_occurrences: MAX_CHAIN_LEN + 1,
            found_common_subsequence: false,
        };
        search.run(before, after, histogram);
        search.success().then_some(search.lcs)
    }

    struct LcsSearch {
        lcs: Lcs,
        min_occurrences: u32,
        found_common_subsequence: bool,
    }

    impl LcsSearch {
        fn run(&mut self, before: &[Token], after: &[Token], histogram: &mut Histogram) {
            let mut position = 0;
            while let Some(&token) = after.get(position as usize) {
                if histogram.occurrence_count(token) != 0 {
                    self.found_common_subsequence = true;
                    if histogram.occurrence_count(token) <= self.min_occurrences {
                        position = self.update_lcs(position, token, histogram, before, after);
                        continue;
                    }
                }
                position += 1;
            }
            histogram.clear();
        }

        fn success(&self) -> bool {
            !self.found_common_subsequence || self.min_occurrences <= MAX_CHAIN_LEN
        }

        fn update_lcs(
            &mut self,
            after_position: u32,
            token: Token,
            histogram: &Histogram,
            before: &[Token],
            after: &[Token],
        ) -> u32 {
            let mut next_after_position = after_position + 1;
            let mut occurrences = histogram.occurrences(token).iter().copied();
            let Some(mut before_position) = occurrences.next() else {
                unreachable!("a counted token must have an occurrence");
            };

            'occurrences: loop {
                let mut occurrence_count = histogram.occurrence_count(token);
                let mut start_before = before_position;
                let mut start_after = after_position;
                while start_before != 0 && start_after != 0 {
                    let before_token = before[start_before as usize - 1];
                    let after_token = after[start_after as usize - 1];
                    if before_token != after_token {
                        break;
                    }
                    start_before -= 1;
                    start_after -= 1;
                    occurrence_count = occurrence_count
                        .min(histogram.occurrence_count(before[start_before as usize]));
                }

                let mut end_before = before_position + 1;
                let mut end_after = after_position + 1;
                while let (Some(&before_token), Some(&after_token)) = (
                    before.get(end_before as usize),
                    after.get(end_after as usize),
                ) {
                    if before_token != after_token {
                        break;
                    }
                    occurrence_count = occurrence_count
                        .min(histogram.occurrence_count(before[end_before as usize]));
                    end_before += 1;
                    end_after += 1;
                }

                next_after_position = next_after_position.max(end_after);
                let len = end_after - start_after;
                debug_assert_eq!(len, end_before - start_before);
                if self.lcs.len < len || self.min_occurrences > occurrence_count {
                    self.min_occurrences = occurrence_count;
                    self.lcs = Lcs {
                        before_start: start_before,
                        after_start: start_after,
                        len,
                    };
                }

                loop {
                    match occurrences.next() {
                        Some(next) if next > end_after => {
                            before_position = next;
                            break;
                        }
                        Some(_) => {}
                        None => break 'occurrences,
                    }
                }
            }
            next_after_position
        }
    }
}

mod list_pool {
    use super::MAX_CHAIN_LEN;

    #[derive(Clone, Default)]
    pub(super) struct ListHandle {
        index: u32,
        generation: u32,
        len: u32,
    }

    const MAX_SIZE_CLASS: SizeClass = size_class_for_length(MAX_CHAIN_LEN - 1);
    const NUM_SIZE_CLASSES: usize = MAX_SIZE_CLASS as usize + 1;
    type SizeClass = u8;

    const fn size_class_size(size_class: SizeClass) -> usize {
        4 << size_class
    }

    const fn size_class_for_length(len: u32) -> SizeClass {
        30 - (len | 3).leading_zeros() as SizeClass
    }

    fn is_size_class_boundary(len: u32) -> bool {
        len > 3 && len.is_power_of_two()
    }

    pub(super) struct ListPool {
        data: Vec<u32>,
        free: [u32; NUM_SIZE_CLASSES],
        generation: u32,
    }

    impl ListPool {
        pub(super) fn new(capacity: u32) -> Self {
            Self {
                data: Vec::with_capacity(capacity as usize),
                free: [u32::MAX; NUM_SIZE_CLASSES],
                generation: 1,
            }
        }

        pub(super) fn clear(&mut self) {
            self.data.clear();
            self.free.fill(u32::MAX);
            self.generation += 1;
        }

        fn allocate(&mut self, size_class: SizeClass) -> usize {
            let head = self.free[size_class as usize];
            if head == u32::MAX {
                let offset = self.data.len();
                self.data
                    .resize(offset + size_class_size(size_class), u32::MAX);
                offset
            } else {
                self.free[size_class as usize] = self.data[head as usize];
                head as usize
            }
        }

        fn free(&mut self, block: usize, size_class: SizeClass) {
            self.data[block] = self.free[size_class as usize];
            self.free[size_class as usize] = block as u32;
        }

        fn mutable_slices(&mut self, block0: usize, block1: usize) -> (&mut [u32], &mut [u32]) {
            if block0 < block1 {
                let (first, second) = self.data.split_at_mut(block1);
                (&mut first[block0..], second)
            } else {
                let (first, second) = self.data.split_at_mut(block0);
                (second, &mut first[block1..])
            }
        }

        fn reallocate(
            &mut self,
            block: usize,
            from: SizeClass,
            to: SizeClass,
            elements: usize,
        ) -> usize {
            let new_block = self.allocate(to);
            let (old, new) = self.mutable_slices(block, new_block);
            new[..elements].copy_from_slice(&old[..elements]);
            self.free(block, from);
            new_block
        }
    }

    impl ListHandle {
        pub(super) fn len(&self, pool: &ListPool) -> u32 {
            if self.generation == pool.generation {
                self.len
            } else {
                0
            }
        }

        pub(super) fn as_slice<'a>(&'a self, pool: &'a ListPool) -> &'a [u32] {
            let index = self.index as usize;
            match self.len(pool) {
                0 => &[],
                1 => std::slice::from_ref(&self.index),
                length => &pool.data[index..index + length as usize],
            }
        }

        pub(super) fn push(&mut self, element: u32, pool: &mut ListPool) {
            let len = self.len(pool);
            match len {
                0 => {
                    self.generation = pool.generation;
                    self.index = element;
                    self.len = 1;
                }
                1 => {
                    let block = pool.allocate(0);
                    pool.data[block] = self.index;
                    pool.data[block + 1] = element;
                    self.index = block as u32;
                    self.len = 2;
                }
                2..=MAX_CHAIN_LEN => {
                    let index = self.index as usize;
                    let block = if is_size_class_boundary(len) {
                        let size_class = size_class_for_length(len);
                        let block =
                            pool.reallocate(index, size_class - 1, size_class, len as usize);
                        self.index = block as u32;
                        block
                    } else {
                        index
                    };
                    pool.data[block + len as usize] = element;
                    self.len += 1;
                }
                _ => {}
            }
        }
    }
}
