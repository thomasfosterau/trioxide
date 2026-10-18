//! Sweeps: the worklist over every unit, in parallel chunks.
//!
//! Units are split once, in `World::unit_order`, into chunks of a fixed size, and each chunk
//! keeps its own bookkeeping (who read what, recordings, TypeScript answers). In a sweep every
//! chunk first works out which of its units are dirty: those it left dirty, and those that
//! read an entry the last merge changed, unless they saw the merged value already. It then runs
//! them in order against an overlay on the tables as they were when the sweep began, seeing
//! its own writes; a write dirties the chunk's units that read the entry. After every chunk
//! has run, their write logs are replayed onto the tables in chunk order. Chunks only read the
//! snapshot, so they run on as many threads as there are; the facts depend on the chunk size,
//! never on the threads or their timing.

use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::NodeRef;
use trioxide_types::{SharedTypeQueries, Type, TypeOracle};

use super::{
    Analyzer, ChunkDeps, Dep, Escape, MAX_ROUNDS, Output, Results, SATURATE_AFTER, State, Stats,
    Types, import_cycles, lexicals, open_modules, patched_builtins,
    tables::{Op, OpCx, Tables, agrees, entries, install, install_all},
};
use crate::{
    AnalysisOptions,
    link::Links,
    threads::Shared,
    value::{Pts, take_lost},
    world::World,
};

/// Units per chunk when the options don't say.
pub const DEFAULT_CHUNK_UNITS: usize = 128;

/// A chunk's bookkeeping, kept across sweeps.
struct Chunk {
    units: Vec<NodeRef>,
    members: FxHashSet<NodeRef>,
    /// Entries its units have read, by entry, over all sweeps.
    readers: FxHashMap<Dep, Vec<NodeRef>>,
    seen: FxHashSet<(Dep, NodeRef)>,
    /// What its units read in the last sweep that ran them.
    last_reads: FxHashSet<(Dep, NodeRef)>,
    /// The entries it wrote in that sweep, as it last saw them.
    last_own: State,
    /// Units still to run.
    pending: FxHashSet<NodeRef>,
    outputs: FxHashMap<NodeRef, Output>,
    ts_cache: FxHashMap<NodeRef, Option<Type>>,
}

impl Chunk {
    fn new(units: Vec<NodeRef>, world: &World<'_>) -> Self {
        let pending = units
            .iter()
            .copied()
            .filter(|unit| !world.module(unit.module).broken)
            .collect();
        Self {
            members: units.iter().copied().collect(),
            units,
            readers: FxHashMap::default(),
            seen: FxHashSet::default(),
            last_reads: FxHashSet::default(),
            last_own: State::default(),
            pending,
            outputs: FxHashMap::default(),
            ts_cache: FxHashMap::default(),
        }
    }

    /// The units to run this sweep: those pending, and readers of entries the last merge
    /// changed that saw something else.
    fn take_dirty(&mut self, changed: &FxHashSet<Dep>, state: &State) -> FxHashSet<NodeRef> {
        let mut dirty = std::mem::take(&mut self.pending);
        if !changed.is_empty() {
            for (dep, units) in &self.readers {
                if !changed.contains(dep) {
                    continue;
                }
                let agreed = agrees(&self.last_own, *dep, state);
                for &unit in units {
                    if !(agreed && self.last_reads.contains(&(*dep, unit))) {
                        dirty.insert(unit);
                    }
                }
            }
        }
        dirty
    }
}

/// What one chunk did in one sweep.
struct Swept {
    log: Vec<Op>,
    /// The entries it wrote (a copy: the chunk keeps its own).
    own: State,
    /// Entries it wrote, or its writes read.
    entries: Vec<Dep>,
    /// Entries its writes changed.
    changed: FxHashSet<Dep>,
    stats: Stats,
}

impl Swept {
    fn idle() -> Self {
        Self {
            log: Vec::new(),
            own: State::default(),
            entries: Vec::new(),
            changed: FxHashSet::default(),
            stats: Stats::default(),
        }
    }
}

/// Splits the units, in order, into chunks of `size`.
fn split(world: &World<'_>, size: usize) -> Vec<Vec<NodeRef>> {
    let mut chunks = Vec::new();
    let mut current: Vec<NodeRef> = Vec::new();
    for &unit in &world.unit_order {
        if current.len() >= size {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(unit);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Everything a chunk's sweep reads.
struct SweepInput<'r, 'a> {
    world: &'r World<'a>,
    links: &'r Links,
    options: &'r AnalysisOptions,
    patched: &'r FxHashSet<String>,
    open_modules: &'r FxHashSet<trioxide_common::ModuleId>,
    lexicals: &'r FxHashMap<NodeRef, Vec<oxc_syntax::symbol::SymbolId>>,
    import_cycles: &'r [usize],
    state: &'r State,
    changed: &'r FxHashSet<Dep>,
    round: usize,
}

/// One sweep of one chunk: finds its dirty units and runs them.
fn sweep_chunk(input: &SweepInput<'_, '_>, chunk: &mut Chunk, mut types: Types<'_>) -> Swept {
    let dirty = chunk.take_dirty(input.changed, input.state);
    if dirty.is_empty() {
        return Swept::idle();
    }
    let ran = run_units(input, chunk, dirty, types.reborrow());
    chunk.pending = ran.dirty;
    chunk.last_own = ran.own.clone();
    chunk.last_reads.clear();
    for (dep, units) in ran.reads {
        for unit in units {
            chunk.last_reads.insert((dep, unit));
            if chunk.seen.insert((dep, unit)) {
                chunk.readers.entry(dep).or_default().push(unit);
            }
        }
    }
    let mut entries: Vec<Dep> = entries(&ran.own).collect();
    entries.extend(ran.op_reads);
    Swept {
        log: ran.log,
        own: ran.own,
        entries,
        changed: ran.changed,
        stats: ran.stats,
    }
}

/// What running a chunk's dirty units left.
struct Ran {
    own: State,
    log: Vec<Op>,
    op_reads: FxHashSet<Dep>,
    changed: FxHashSet<Dep>,
    reads: FxHashMap<Dep, Vec<NodeRef>>,
    dirty: FxHashSet<NodeRef>,
    stats: Stats,
}

/// Runs the chunk's `dirty` units, in order.
fn run_units<'w>(
    input: &'w SweepInput<'w, '_>,
    chunk: &'w mut Chunk,
    dirty: FxHashSet<NodeRef>,
    types: Types<'w>,
) -> Ran {
    let mut analyzer = Analyzer {
        world: input.world,
        links: input.links,
        options: input.options,
        patched: input.patched,
        open_modules: input.open_modules,
        lexicals: input.lexicals,
        import_cycles: input.import_cycles,
        round: input.round,
        tables: Tables::overlay(input.state),
        log: Vec::new(),
        op_reads: FxHashSet::default(),
        changed: FxHashSet::default(),
        output: Output::default(),
        outputs: &mut chunk.outputs,
        deps: std::cell::RefCell::new(ChunkDeps {
            current: None,
            current_reads: FxHashSet::default(),
            readers: &chunk.readers,
            reads: FxHashMap::default(),
            seen: FxHashSet::default(),
            members: &chunk.members,
            dirty,
        }),
        ts_cache: &mut chunk.ts_cache,
        types,
        stats: Stats::default(),
    };
    let mut deferred = Vec::new();
    for &unit in &chunk.units {
        if !analyzer.deps.get_mut().dirty.remove(&unit) {
            continue;
        }
        let info = input.world.unit(unit);
        // In the first sweep, a function no caller has reached yet would only run on
        // `never` arguments, and again once one does: it waits a sweep for its callers.
        if input.round == 0 && analyzer.uncalled(info) {
            deferred.push(unit);
            continue;
        }
        analyzer.stats.interpretations += 1;
        analyzer.interpret(info);
    }
    analyzer.deps.get_mut().dirty.extend(deferred);
    let deps = analyzer.deps.into_inner();
    Ran {
        own: analyzer.tables.own,
        log: analyzer.log,
        op_reads: analyzer.op_reads,
        changed: analyzer.changed,
        reads: deps.reads,
        dirty: deps.dirty,
        stats: analyzer.stats,
    }
}

/// Runs the interpreter to a fixed point.
pub fn run(
    world: &World<'_>,
    links: &Links,
    oracle: &mut dyn TypeOracle,
    options: &AnalysisOptions,
) -> Results {
    let started = std::time::Instant::now();
    // Sites joins on this thread dropped before (another analysis) don't concern this one.
    let _ = take_lost();
    let patched = patched_builtins(world);
    let open_modules = open_modules(world, options);
    let lexicals = lexicals(world);
    let import_cycles = import_cycles(world);
    let size = match options.chunk_units {
        0 => DEFAULT_CHUNK_UNITS,
        size => size,
    };
    let mut chunks: Vec<Chunk> = split(world, size)
        .into_iter()
        .map(|units| Chunk::new(units, world))
        .collect();
    let threads = crate::threads::count(options);
    let shared_types = oracle.shared().is_some();

    let mut state = State::default();
    let mut changed: FxHashSet<Dep> = FxHashSet::default();
    let mut stats = Stats {
        units: world.unit_order.len(),
        chunks: chunks.len(),
        ..Stats::default()
    };
    let mut round = 0;
    let mut converged = false;
    let mut saturated = 0;
    // Freed once the sweeps are over, not while workers need the cores.
    let mut spent_logs: Vec<Vec<Vec<Op>>> = Vec::new();
    while round < MAX_ROUNDS {
        let input = SweepInput {
            world,
            links,
            options,
            patched: &patched,
            open_modules: &open_modules,
            lexicals: &lexicals,
            import_cycles: &import_cycles,
            state: &state,
            changed: &changed,
            round,
        };
        let swept: Vec<Swept> = if shared_types {
            let shared = oracle
                .shared()
                .expect("the oracle answered from threads before");
            sweep_shared(&input, &mut chunks, shared, threads)
        } else {
            chunks
                .iter_mut()
                .map(|chunk| sweep_chunk(&input, chunk, Types::Exclusive(&mut *oracle)))
                .collect()
        };
        let ran = swept.iter().any(|swept| swept.stats.interpretations > 0);
        for swept in &swept {
            stats.interpretations += swept.stats.interpretations;
            stats.oracle_queries += swept.stats.oracle_queries;
            stats.oracle_time += swept.stats.oracle_time;
        }
        if !ran {
            converged = true;
            break;
        }

        // Replay the writes, in chunk order. A chunk whose entries (written, or read by its
        // writes) no earlier chunk's writes or replay touched would replay to exactly its
        // overlay: install that.
        let merging = std::time::Instant::now();
        let mut global = Tables::global(std::mem::take(&mut state));
        let cx = OpCx {
            world,
            links,
            round,
        };
        let mut merged: FxHashSet<Dep> = FxHashSet::with_capacity_and_hasher(
            swept.iter().map(|swept| swept.entries.len()).sum(),
            rustc_hash::FxBuildHasher,
        );
        let mut next_changed: FxHashSet<Dep> = FxHashSet::default();
        // When no two chunks share an entry, no chunk replays, so every chunk is fast.
        let mut all_fast = true;
        for swept in &swept {
            all_fast &= swept.entries.iter().all(|dep| !merged.contains(dep));
            merged.extend(swept.entries.iter().copied());
        }
        let mut spent = Vec::with_capacity(swept.len());
        if all_fast {
            // No chunk saw another's writes: install them all, a table per thread.
            let mut owns = Vec::with_capacity(swept.len());
            let mut changes = Vec::with_capacity(swept.len());
            for swept in swept {
                changes.push(swept.changed);
                owns.push(swept.own);
                spent.push(swept.log);
            }
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    next_changed.reserve(changes.iter().map(FxHashSet::len).sum());
                    for change in changes {
                        next_changed.extend(change);
                    }
                });
                install_all(owns, &mut global.own);
            });
        } else {
            // A replay can change entries outside the chunk's own (escaping a parent class,
            // following new props), so whether a chunk is fast is only known once every
            // chunk before it is merged.
            merged.clear();
            for swept in swept {
                if swept.entries.iter().all(|dep| !merged.contains(dep)) {
                    install(swept.own, &mut global.own);
                    next_changed.extend(swept.changed);
                } else {
                    for op in &swept.log {
                        global.apply(cx, op);
                    }
                    merged.extend(global.touched.iter().copied());
                    merged.extend(global.reads.drain(..));
                    next_changed.extend(global.touched.drain(..));
                }
                merged.extend(swept.entries);
                spent.push(swept.log);
            }
        }
        spent_logs.push(spent);
        // Sites the replayed joins dropped into "any object" are no longer followed.
        let mut lost = take_lost();
        lost.sort_unstable();
        lost.dedup();
        for site in lost {
            global.apply(cx, &Op::Escape(Pts::site(site), Escape::Unknown));
        }
        next_changed.extend(global.touched.drain(..));
        global.reads.clear();
        let saturate_after = match options.saturate_after {
            0 => SATURATE_AFTER,
            rounds => rounds,
        };
        if round >= saturate_after {
            give_up_changed(&mut global, cx, &mut next_changed);
        }
        changed = next_changed;
        saturated += global.given_up;
        state = global.own;
        stats.merge_time += merging.elapsed();
        round += 1;
    }
    stats.rounds = round;
    stats.saturated = saturated;
    stats.total_time = started.elapsed();
    let mut outputs = FxHashMap::default();
    for chunk in &mut chunks {
        outputs.extend(std::mem::take(&mut chunk.outputs));
    }
    // The bookkeeping is big and done with: free it off this thread.
    std::thread::spawn(move || drop((spent_logs, chunks)));
    Results {
        state,
        outputs,
        converged,
        stats,
    }
}

/// Gives up every entry the merge changed (see [`SATURATE_AFTER`]), in a fixed order, and
/// the ones that changes, until none is left; what changed goes into `changed`.
fn give_up_changed(global: &mut Tables<'_>, cx: OpCx<'_, '_>, changed: &mut FxHashSet<Dep>) {
    let mut done: FxHashSet<Dep> = FxHashSet::default();
    loop {
        let mut pending: Vec<Dep> = changed
            .iter()
            .filter(|dep| !done.contains(dep))
            .copied()
            .collect();
        if pending.is_empty() {
            return;
        }
        pending.sort_unstable();
        for dep in pending {
            done.insert(dep);
            global.apply(cx, &Op::GiveUp(dep));
        }
        changed.extend(global.touched.drain(..));
    }
}

/// Sweeps every chunk on the oracle's workers (at most `threads` of them), returning what
/// each did, in chunk order.
fn sweep_shared(
    input: &SweepInput<'_, '_>,
    chunks: &mut [Chunk],
    shared: &dyn SharedTypeQueries,
    threads: usize,
) -> Vec<Swept> {
    if threads <= 1 || shared.workers() <= 1 || chunks.len() <= 1 {
        return chunks
            .iter_mut()
            .map(|chunk| sweep_chunk(input, chunk, Types::Shared(shared)))
            .collect();
    }
    let work: Vec<Mutex<Option<&mut Chunk>>> = chunks
        .iter_mut()
        .map(|chunk| Mutex::new(Some(chunk)))
        .collect();
    let slots: Vec<Mutex<Option<Swept>>> = work.iter().map(|_| Mutex::new(None)).collect();
    let workers = threads.min(shared.workers());
    // Each worker owns a contiguous run of chunks, so a module's queries go to the same
    // checker every sweep (a checker answers faster about files it has checked); a worker
    // that runs out takes chunks from the end of the others' runs.
    let share = work.len().div_ceil(workers);
    let next: Vec<AtomicUsize> = (0..workers)
        .map(|worker| AtomicUsize::new(worker * share))
        .collect();
    let input = Shared(input);
    shared.run_workers(&|worker, queries| {
        if worker >= workers {
            return;
        }
        let input = input.get();
        let mut sweep = |at: usize| {
            let Some(chunk) = work[at]
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            else {
                return;
            };
            let swept = sweep_chunk(input, chunk, Types::Worker(&mut *queries));
            *slots[at]
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(swept);
        };
        let end = ((worker + 1) * share).min(work.len());
        loop {
            let at = next[worker].fetch_add(1, Ordering::Relaxed);
            if at >= end {
                break;
            }
            sweep(at);
        }
        // Help the others, from the back of their runs.
        for other in (0..workers).filter(|&other| other != worker) {
            let end = ((other + 1) * share).min(work.len());
            for at in (other * share..end).rev() {
                if next[other].load(Ordering::Relaxed) > at {
                    break;
                }
                sweep(at);
            }
        }
    });
    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .expect("every chunk was swept")
        })
        .collect()
}
