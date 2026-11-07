use super::types::MemoryBreakdown;

impl MemoryBreakdown {
    pub fn total_bytes(self) -> u64 {
        self.wired
            .saturating_add(self.active)
            .saturating_add(self.compressed)
            .saturating_add(self.inactive)
            .saturating_add(self.speculative)
            .saturating_add(self.free)
            .saturating_add(self.other)
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct MemoryPageCounts {
    pub wired: u64,
    pub active: u64,
    pub compressed: u64,
    pub inactive: u64,
    pub purgeable: u64,
    pub speculative: u64,
    pub file_backed: u64,
    pub free: u64,
}

#[cfg(any(target_os = "macos", test))]
fn take_pages(count: u64, remaining: &mut u64, page_size: u64) -> u64 {
    let accepted = count.min(*remaining);
    *remaining -= accepted;
    accepted.saturating_mul(page_size)
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn breakdown(
    counts: MemoryPageCounts,
    page_size: u64,
    total_bytes: u64,
) -> MemoryBreakdown {
    let pages = total_bytes / page_size.max(1);
    let sampled_free = counts.free.min(pages);
    let mut remaining = pages - sampled_free;
    let speculative = counts.speculative.min(sampled_free);
    let wired = take_pages(counts.wired, &mut remaining, page_size);
    let active = take_pages(counts.active, &mut remaining, page_size);
    let compressed = take_pages(counts.compressed, &mut remaining, page_size);
    let inactive = take_pages(counts.inactive, &mut remaining, page_size);

    // VM free includes speculative pages. Split that known intersection once.
    // File-backed and purgeable counters overlap the queues, but do not identify
    // their intersections. Keep them as non-additive "of which" attributes instead
    // of guessing which queue to subtract from. Other holds unclassified memory
    // and sub-page bytes. Capping queue counts handles inconsistent samples.
    MemoryBreakdown {
        wired,
        active,
        compressed,
        inactive,
        purgeable: counts.purgeable.min(pages).saturating_mul(page_size),
        speculative: speculative.saturating_mul(page_size),
        file_cache: counts.file_backed.min(pages).saturating_mul(page_size),
        free: (sampled_free - speculative).saturating_mul(page_size),
        other: total_bytes - (pages - remaining) * page_size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_attributes_do_not_remove_active_queue_pages() {
        let m = breakdown(
            MemoryPageCounts {
                wired: 1,
                active: 4,
                inactive: 4,
                file_backed: 4,
                free: 1,
                ..Default::default()
            },
            4096,
            10 * 4096,
        );
        assert_eq!(m.active, 4 * 4096);
        assert_eq!(m.inactive, 4 * 4096);
        assert_eq!(m.file_cache, 4 * 4096);
        assert_eq!(m.total_bytes(), 10 * 4096);
    }

    #[test]
    fn speculative_pages_are_removed_from_free_only() {
        let m = breakdown(
            MemoryPageCounts {
                wired: 1,
                active: 4,
                inactive: 2,
                speculative: 1,
                file_backed: 1,
                free: 3,
                ..Default::default()
            },
            4096,
            10 * 4096,
        );
        assert_eq!(m.free, 2 * 4096);
        assert_eq!(m.active, 4 * 4096);
        assert_eq!(m.inactive, 2 * 4096);
        assert_eq!(m.speculative, 4096);
        assert_eq!(m.total_bytes(), 10 * 4096);
    }

    #[test]
    fn active_purgeable_attribute_is_visible_without_changing_queue_totals() {
        let m = breakdown(
            MemoryPageCounts {
                wired: 1,
                active: 6,
                purgeable: 2,
                free: 3,
                ..Default::default()
            },
            4096,
            10 * 4096,
        );
        assert_eq!(m.purgeable, 2 * 4096);
        assert_eq!(m.active, 6 * 4096);
        assert_eq!(m.total_bytes(), 10 * 4096);
    }

    #[test]
    fn page_groups_remove_overlaps_and_add_up_to_total() {
        let breakdown = breakdown(
            MemoryPageCounts {
                wired: 2,
                active: 5,
                compressed: 1,
                inactive: 6,
                purgeable: 2,
                speculative: 1,
                file_backed: 4,
                free: 3,
            },
            4096,
            24 * 4096,
        );

        assert_eq!(breakdown.wired, 2 * 4096);
        assert_eq!(breakdown.active, 5 * 4096);
        assert_eq!(breakdown.compressed, 4096);
        assert_eq!(breakdown.inactive, 6 * 4096);
        assert_eq!(breakdown.purgeable, 2 * 4096);
        assert_eq!(breakdown.speculative, 4096);
        assert_eq!(breakdown.file_cache, 4 * 4096);
        assert_eq!(breakdown.free, 2 * 4096);
        assert_eq!(breakdown.other, 7 * 4096);
        assert_eq!(breakdown.total_bytes(), 24 * 4096);
    }

    #[test]
    fn page_groups_cap_overreported_counters_at_total() {
        let breakdown = breakdown(
            MemoryPageCounts {
                wired: 8,
                active: 8,
                compressed: 8,
                inactive: 8,
                purgeable: 8,
                speculative: 8,
                file_backed: 8,
                free: 8,
            },
            4096,
            10 * 4096,
        );

        assert_eq!(breakdown.total_bytes(), 10 * 4096);
        assert_eq!(breakdown.free, 0);
        assert_eq!(breakdown.speculative, 8 * 4096);
    }

    #[test]
    fn unclassified_memory_and_subpage_bytes_stay_out_of_free() {
        let m = breakdown(
            MemoryPageCounts {
                active: 1,
                free: 1,
                ..Default::default()
            },
            4096,
            3 * 4096 + 7,
        );
        assert_eq!(m.active, 4096);
        assert_eq!(m.free, 4096);
        assert_eq!(m.other, 4096 + 7);
        assert_eq!(m.total_bytes(), 3 * 4096 + 7);
    }
}
