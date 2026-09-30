//! How much the indexer may use, from the memory and CPU budget of the
//! process. The tiers follow the Go plugin: a small budget gets a small writer
//! and one thread, a large one gets more of both.

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// Sizes of one indexing run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizing {
    /// Heap of the tantivy writer in MiB, shared by its threads.
    pub heap_mb: usize,
    /// Indexing threads of the writer and parser threads of the pipeline.
    pub threads: usize,
    /// Lines handed to a parser thread at once.
    pub batch_lines: usize,
    /// Documents between two commits of a bulk import.
    pub commit_every: u64,
}

/// Tiers by memory budget: under 1 GiB a 50 MB heap and one thread, from 4 GiB
/// a 1 GiB heap and four threads, 200 MB and two threads in between. An
/// unknown budget takes the middle tier. Threads never exceed the CPUs.
pub fn sizing(memory: Option<u64>, cpus: usize) -> Sizing {
    let (heap_mb, threads, batch_lines, commit_every) = match memory {
        Some(m) if m < GIB => (50, 1, 1000, 200_000),
        Some(m) if m >= 4 * GIB => (1024, 4, 5000, 500_000),
        _ => (200, 2, 2000, 300_000),
    };
    Sizing { heap_mb, threads: threads.min(cpus.max(1)), batch_lines, commit_every }
}

/// Sizing of this process.
pub fn current() -> Sizing {
    sizing(crate::sys::available_memory(), crate::sys::available_cpus())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_follow_the_memory_budget() {
        assert_eq!(
            sizing(Some(512 * MIB), 8),
            Sizing { heap_mb: 50, threads: 1, batch_lines: 1000, commit_every: 200_000 }
        );
        assert_eq!(sizing(Some(2 * GIB), 8).heap_mb, 200);
        assert_eq!(sizing(Some(2 * GIB), 8).threads, 2);
        assert_eq!(sizing(Some(8 * GIB), 8).threads, 4);
        assert_eq!(sizing(None, 8).heap_mb, 200);
    }

    #[test]
    fn threads_are_capped_by_the_cpus() {
        assert_eq!(sizing(Some(16 * GIB), 2).threads, 2);
        assert_eq!(sizing(Some(16 * GIB), 0).threads, 1);
    }
}
