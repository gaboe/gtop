use super::types::MemoryBreakdown;

impl MemoryBreakdown {
    pub fn used_bytes(self) -> u64 {
        self.wired
            .saturating_add(self.active)
            .saturating_add(self.compressed)
    }

    pub fn reclaimable_bytes(self) -> u64 {
        self.inactive
            .saturating_add(self.purgeable)
            .saturating_add(self.speculative)
            .saturating_add(self.file_cache)
    }

    pub fn total_bytes(self) -> u64 {
        self.used_bytes()
            .saturating_add(self.reclaimable_bytes())
            .saturating_add(self.free)
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
    let file_backed_active = counts.file_backed.min(counts.active);
    let file_backed_inactive = counts
        .file_backed
        .saturating_sub(file_backed_active)
        .min(counts.inactive);
    let inactive_after_file_cache = counts.inactive.saturating_sub(file_backed_inactive);
    let purgeable_inactive = counts.purgeable.min(inactive_after_file_cache);
    let speculative_inactive = counts
        .speculative
        .min(inactive_after_file_cache.saturating_sub(purgeable_inactive));

    let sampled_free = counts.free.min(pages);
    let mut remaining = pages - sampled_free;

    let wired = take_pages(counts.wired, &mut remaining, page_size);
    let active = take_pages(
        counts.active.saturating_sub(file_backed_active),
        &mut remaining,
        page_size,
    );
    let compressed = take_pages(counts.compressed, &mut remaining, page_size);
    let inactive = take_pages(
        inactive_after_file_cache
            .saturating_sub(purgeable_inactive)
            .saturating_sub(speculative_inactive),
        &mut remaining,
        page_size,
    );
    let purgeable = take_pages(counts.purgeable, &mut remaining, page_size);
    let speculative = take_pages(counts.speculative, &mut remaining, page_size);
    let file_cache = take_pages(counts.file_backed, &mut remaining, page_size);

    // File-backed pages overlap active and inactive counts. Purgeable and speculative pages
    // overlap inactive counts. Assign those subgroups once. Free reserves the sampled free
    // pages and absorbs any remaining physical memory so the rows cannot exceed the total.
    MemoryBreakdown {
        wired,
        active,
        compressed,
        inactive,
        purgeable,
        speculative,
        file_cache,
        free: total_bytes - (pages - sampled_free - remaining) * page_size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(breakdown.active, 1 * 4096);
        assert_eq!(breakdown.compressed, 1 * 4096);
        assert_eq!(breakdown.inactive, 3 * 4096);
        assert_eq!(breakdown.purgeable, 2 * 4096);
        assert_eq!(breakdown.speculative, 1 * 4096);
        assert_eq!(breakdown.file_cache, 4 * 4096);
        assert_eq!(breakdown.free, 10 * 4096);
        assert_eq!(breakdown.used_bytes(), 4 * 4096);
        assert_eq!(breakdown.reclaimable_bytes(), 10 * 4096);
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
        assert_eq!(breakdown.free, 8 * 4096);
    }
}
