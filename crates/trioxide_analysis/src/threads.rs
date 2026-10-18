//! Running analysis work on several threads.

use crate::AnalysisOptions;

/// Lets read-only analysis inputs (references, so `Copy`) cross into worker threads.
///
/// The world holds oxc ASTs and semantic data, which are `!Sync` only because of `Cell`s for
/// ids that semantic analysis fills in while parsing; nothing writes them afterwards, the
/// world's own cache is behind a mutex, and the analysis never allocates in the modules'
/// arenas. So references to them may be shared once the world is built. The tables, facts and
/// summaries the sweeps and the summary read are plain data, `!Sync` only through the `Rc`s
/// and `Cell`s of values no thread changes while they are shared.
pub(crate) struct Shared<T>(T);

// SAFETY: a `Shared` is only made through `Shared::new`, whose callers promise that nothing
// reachable from the value is written (or has its reference counts changed) while it is shared.
unsafe impl<T: Copy> Sync for Shared<T> {}
// SAFETY: as above.
unsafe impl<T: Copy> Send for Shared<T> {}

impl<T: Copy> Shared<T> {
    /// Wraps `value` for reading on other threads.
    ///
    /// # Safety
    ///
    /// Until the wrapper is dropped, nothing reachable from `value` may be written, by any
    /// thread: no `Cell` set, no `Rc` cloned or dropped, nothing behind a reference mutated.
    pub unsafe fn new(value: T) -> Self {
        Self(value)
    }

    /// The wrapped value. (A method, so closures capture the wrapper, not the value.)
    pub fn get(&self) -> T {
        self.0
    }
}

/// Drops `value` on a background thread, so freeing big tables doesn't hold up the caller.
///
/// One thread frees for every analysis in the process, behind a queue of one: a long-running
/// host (the Vite dev server) that analyses faster than the frees finish waits for the
/// previous one instead of piling up threads and garbage.
pub fn drop_later<T: Send + 'static>(value: T) {
    type Garbage = Box<dyn std::any::Any + Send>;
    static QUEUE: std::sync::OnceLock<Option<std::sync::mpsc::SyncSender<Garbage>>> =
        std::sync::OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Garbage>(1);
        std::thread::Builder::new()
            .name("trioxide-drop".into())
            .spawn(move || {
                for garbage in receiver {
                    drop(garbage);
                }
            })
            .ok()
            .map(|_| sender)
    });
    match queue {
        Some(queue) => {
            // If the thread is gone, the value comes back and is freed here.
            if let Err(std::sync::mpsc::SendError(value)) = queue.send(Box::new(value)) {
                drop(value);
            }
        }
        None => drop(value),
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
