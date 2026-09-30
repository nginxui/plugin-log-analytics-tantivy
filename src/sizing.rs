//! How much the indexer may use, from the memory and CPU budget of the
//! process. The writer heap is a share of the memory budget, and the threads,
//! batches and commits grow with it.

const MIB: u64 = 1024 * 1024;

/// Share of the memory budget the writer heap takes. The rest covers merges,
/// parsers, queries and the host process.
const HEAP_SHARE: u64 = 4;
/// Bounds of the writer heap in MiB. tantivy needs 15 MB per thread.
const MIN_HEAP_MB: usize = 20;
const MAX_HEAP_MB: usize = 1024;
/// Heap of one writer thread in MiB; a smaller heap per thread makes many
/// small segments to merge.
const HEAP_PER_THREAD_MB: usize = 48;
const MAX_THREADS: usize = 4;
/// Budget assumed when it cannot be read.
const UNKNOWN_BUDGET: u64 = 1024 * MIB;

/// Sizes of one indexing run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizing {
    /// Heap of the tantivy writer in MiB, shared by its threads.
    pub heap_mb: usize,
    /// Indexing threads of the writer and parser threads of the pipeline.
    pub threads: usize,
    /// Threads that merge segments in the background.
    pub merge_threads: usize,
    /// Lines handed to a parser thread at once.
    pub batch_lines: usize,
    /// Documents between two commits of a bulk import.
    pub commit_every: u64,
}

/// Sizing for a memory budget in bytes and a number of CPUs. The heap is a
/// quarter of the budget within 20 MiB and 1 GiB, one thread per 48 MiB of
/// heap up to four and never more than the CPUs.
pub fn sizing(memory: Option<u64>, cpus: usize) -> Sizing {
    let budget_mb = (memory.unwrap_or(UNKNOWN_BUDGET) / MIB / HEAP_SHARE) as usize;
    let heap_mb = budget_mb.clamp(MIN_HEAP_MB, MAX_HEAP_MB);
    let threads = (heap_mb / HEAP_PER_THREAD_MB).clamp(1, MAX_THREADS.min(cpus.max(1)));
    Sizing {
        heap_mb,
        threads,
        merge_threads: threads,
        batch_lines: (heap_mb * 20).clamp(500, 5000),
        commit_every: (heap_mb as u64 * 2000).clamp(200_000, 500_000),
    }
}

/// Sizing of this process.
pub fn current() -> Sizing {
    sizing(crate::sys::available_memory(), crate::sys::available_cpus())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_is_a_share_of_the_budget() {
        assert_eq!(
            sizing(Some(128 * MIB), 8),
            Sizing { heap_mb: 32, threads: 1, merge_threads: 1, batch_lines: 640, commit_every: 200_000 }
        );
        assert_eq!(sizing(Some(64 * MIB), 8).heap_mb, 20);
        assert_eq!(sizing(Some(512 * MIB), 8).heap_mb, 128);
        assert_eq!(sizing(Some(512 * MIB), 8).threads, 2);
        assert_eq!(sizing(Some(1024 * MIB), 8).threads, 4);
        assert_eq!(sizing(Some(16 * 1024 * MIB), 8).heap_mb, 1024);
        assert_eq!(sizing(Some(16 * 1024 * MIB), 8).batch_lines, 5000);
        assert_eq!(sizing(Some(16 * 1024 * MIB), 8).commit_every, 500_000);
        assert_eq!(sizing(None, 8).heap_mb, 256);
    }

    #[test]
    fn threads_are_capped_by_the_cpus() {
        assert_eq!(sizing(Some(16 * 1024 * MIB), 2).threads, 2);
        assert_eq!(sizing(Some(16 * 1024 * MIB), 0).threads, 1);
    }
}
