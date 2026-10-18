//! Running analysis work on several threads.

use crate::AnalysisOptions;

/// Lets read-only analysis inputs cross into worker threads.
///
/// The world holds oxc ASTs and semantic data, which are `!Sync` only because of `Cell`s for
/// ids that semantic analysis fills in while parsing; nothing writes them afterwards, the
/// world's own cache is behind a mutex, and the analysis never allocates in the modules'
/// arenas. So references to them may be shared once the world is built.
pub struct Shared<T>(pub T);

// SAFETY: see the type's documentation; the wrapped values are only read.
unsafe impl<T> Sync for Shared<T> {}
// SAFETY: as above.
unsafe impl<T> Send for Shared<T> {}

impl<T: Copy> Shared<T> {
    /// The wrapped value. (A method, so closures capture the wrapper, not the value.)
    pub fn get(&self) -> T {
        self.0
    }
}

/// How many threads the options ask for (0: one per core).
pub fn count(options: &AnalysisOptions) -> usize {
    match options.threads {
        0 => std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
        threads => threads,
    }
}

/// `f` over `0..len`, split into contiguous ranges on up to `threads` threads (none for
/// small inputs), with the results in order.
pub fn map_ranges<R: Send>(
    len: usize,
    threads: usize,
    f: impl Fn(std::ops::Range<usize>) -> Vec<R> + Sync,
) -> Vec<R> {
    /// Below this many items a thread costs more than it saves.
    const MIN_PER_THREAD: usize = 16;
    let threads = threads.min(len / MIN_PER_THREAD).max(1);
    if threads == 1 {
        return f(0..len);
    }
    let per_thread = len.div_ceil(threads);
    std::thread::scope(|scope| {
        let f = &f;
        let handles: Vec<_> = (0..threads)
            .map(|thread| {
                let start = (thread * per_thread).min(len);
                let end = (start + per_thread).min(len);
                scope.spawn(move || f(start..end))
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("a worker panicked"))
            .collect()
    })
}
