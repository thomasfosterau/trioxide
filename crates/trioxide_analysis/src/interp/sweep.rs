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

use std::sync::Mutex;

use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_common::NodeRef;
use trioxide_types::{SharedTypeQueries, Type, TypeOracle};

use super::{
    Analyzer, ChunkDeps, Dep, Escape, MAX_ROUNDS, Output, Results, State, Stats, Types,
    import_cycles, lexicals, open_modules, patched_builtins,
    tables::{Op, OpCx, Slot, SlotChanges, Tables, agrees, entries, install, install_all},
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
        let mut visit = |dep: &Dep, units: &Vec<NodeRef>| {
            let agreed = agrees(&self.last_own, *dep, state);
            for &unit in units {
                if !(agreed && self.last_reads.contains(&(*dep, unit))) {
                    dirty.insert(unit);
                }
            }
        };
        if changed.len() < self.readers.len() {
            for dep in changed {
                if let Some(units) = self.readers.get(dep) {
                    visit(dep, units);
                }
            }
        } else {
            for (dep, units) in &self.readers {
                if changed.contains(dep) {
                    visit(dep, units);
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
    /// Heap properties they changed.
    changed_slots: FxHashSet<Slot>,
    stats: Stats,
}

impl Swept {
    fn idle() -> Self {
        Self {
            log: Vec::new(),
            own: State::default(),
            entries: Vec::new(),
            changed: FxHashSet::default(),
            changed_slots: FxHashSet::default(),
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
    instance_fields: &'r FxHashMap<NodeRef, FxHashSet<String>>,
    optimistic: bool,
    import_cycles: &'r [usize],
    state: &'r State,
    /// How many merges changed each entry so far.
    changes: &'r FxHashMap<Dep, u32>,
    slot_changes: &'r SlotChanges,
}

/// Takes every chunk's dirty units for this sweep, and says whether any of them is a unit
/// its callers have reached: one that may call the functions still waiting for a caller.
fn take_dirty(
    chunks: &mut [Chunk],
    world: &World<'_>,
    changed: &FxHashSet<Dep>,
    state: &State,
) -> (Vec<FxHashSet<NodeRef>>, bool) {
    let tables = Tables::overlay(state);
    let dirty: Vec<FxHashSet<NodeRef>> = chunks
        .iter_mut()
        .map(|chunk| chunk.take_dirty(changed, state))
        .collect();
    let busy = dirty
        .iter()
        .flatten()
        .any(|&unit| !super::uncalled(world, &tables, world.unit(unit)));
    (dirty, busy)
}

/// One sweep of one chunk: runs its `dirty` units.
fn sweep_chunk(
    input: &SweepInput<'_, '_>,
    chunk: &mut Chunk,
    dirty: FxHashSet<NodeRef>,
    busy: bool,
    mut types: Types<'_>,
) -> Swept {
    if dirty.is_empty() {
        return Swept::idle();
    }
    let ran = run_units(input, chunk, dirty, busy, types.reborrow());
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
        changed_slots: ran.changed_slots,
        stats: ran.stats,
    }
}

/// What running a chunk's dirty units left.
struct Ran {
    own: State,
    log: Vec<Op>,
    op_reads: FxHashSet<Dep>,
    changed: FxHashSet<Dep>,
    changed_slots: FxHashSet<Slot>,
    reads: FxHashMap<Dep, Vec<NodeRef>>,
    dirty: FxHashSet<NodeRef>,
    stats: Stats,
}

/// Runs the chunk's `dirty` units, in order.
fn run_units<'w>(
    input: &'w SweepInput<'w, '_>,
    chunk: &'w mut Chunk,
    dirty: FxHashSet<NodeRef>,
    busy: bool,
    types: Types<'w>,
) -> Ran {
    let mut analyzer = Analyzer {
        world: input.world,
        links: input.links,
        options: input.options,
        patched: input.patched,
        open_modules: input.open_modules,
        lexicals: input.lexicals,
        instance_fields: input.instance_fields,
        optimistic: input.optimistic,
        import_cycles: input.import_cycles,
        changes: input.changes,
        slot_changes: input.slot_changes,
        tables: Tables::overlay(input.state),
        log: Vec::new(),
        op_reads: FxHashSet::default(),
        changed: FxHashSet::default(),
        changed_slots: FxHashSet::default(),
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
        // A function no caller has reached yet would only run on `never` arguments, and again
        // once one does: it waits while units that may call it run (their calls in other
        // chunks are only seen at the merge). With nothing else left to run, it runs as it is.
        if busy && analyzer.uncalled(info) {
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
        changed_slots: analyzer.changed_slots,
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
    let instance_fields = super::instance_fields(world);
    let mut optimistic = true;
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
    let mut changes: FxHashMap<Dep, u32> = FxHashMap::default();
    let mut slot_changes = SlotChanges::default();
    let saturate_after = super::saturate_after(options);
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
            instance_fields: &instance_fields,
            optimistic,
            import_cycles: &import_cycles,
            state: &state,
            changes: &changes,
            slot_changes: &slot_changes,
        };
        let (dirty, busy) = take_dirty(&mut chunks, world, &changed, &state);
        let swept: Vec<Swept> = if shared_types {
            let shared = oracle
                .shared()
                .expect("the oracle answered from threads before");
            sweep_shared(&input, &mut chunks, dirty, busy, shared, threads)
        } else {
            chunks
                .iter_mut()
                .zip(dirty)
                .map(|(chunk, dirty)| {
                    sweep_chunk(&input, chunk, dirty, busy, Types::Exclusive(&mut *oracle))
                })
                .collect()
        };
        let ran = swept.iter().any(|swept| swept.stats.interpretations > 0);
        for swept in &swept {
            stats.interpretations += swept.stats.interpretations;
            stats.oracle_queries += swept.stats.oracle_queries;
            stats.oracle_time += swept.stats.oracle_time;
            saturated += swept.stats.saturated;
        }
        if !ran {
            if optimistic {
                // Settled with unwritten fields read as `never`: from here they read as
                // missing, and the units that read one run again.
                optimistic = false;
                let mut again = false;
                for chunk in &mut chunks {
                    for (&unit, output) in &chunk.outputs {
                        if output.optimistic {
                            chunk.pending.insert(unit);
                            again = true;
                        }
                    }
                }
                if again {
                    changed.clear();
                    continue;
                }
            }
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
            changes: &changes,
            slot_changes: &slot_changes,
        };
        let mut merged: FxHashSet<Dep> = FxHashSet::with_capacity_and_hasher(
            swept.iter().map(|swept| swept.entries.len()).sum(),
            rustc_hash::FxBuildHasher,
        );
        let mut next_changed: FxHashSet<Dep> = FxHashSet::default();
        let mut next_slots: FxHashSet<Slot> = FxHashSet::default();
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
                next_slots.extend(swept.changed_slots);
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
                    next_slots.extend(swept.changed_slots);
                } else {
                    for op in &swept.log {
                        global.apply(cx, op);
                    }
                    merged.extend(global.touched.iter().copied());
                    merged.extend(global.reads.drain(..));
                    next_changed.extend(global.touched.drain(..));
                    next_slots.extend(global.touched_slots.drain(..));
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
        give_up_changed(&mut global, cx, &mut next_changed, saturate_after);
        for &dep in &next_changed {
            *changes.entry(dep).or_default() += 1;
        }
        global.touched_slots.clear();
        for slot in next_slots {
            slot_changes.count(slot);
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
    crate::threads::drop_later((spent_logs, chunks));
    Results {
        state,
        outputs,
        converged,
        stats,
    }
}

/// Gives up every entry the merge changed that had changed in `saturate_after` merges before
/// (see [`SATURATE_AFTER`]), in a fixed order, and the ones that changes that had too, until
/// none is left; what changed goes into `changed`.
fn give_up_changed(
    global: &mut Tables<'_>,
    cx: OpCx<'_, '_>,
    changed: &mut FxHashSet<Dep>,
    saturate_after: u32,
) {
    let mut done: FxHashSet<Dep> = FxHashSet::default();
    loop {
        let mut pending: Vec<Dep> = changed
            .iter()
            .filter(|dep| {
                !done.contains(dep)
                    && cx
                        .changes
                        .get(dep)
                        .is_some_and(|&count| count >= saturate_after)
            })
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
///
/// Each worker holds a checker of its own and sweeps one contiguous run of chunks, in order.
/// The runs are cut from the chunks' dirty units alone, so which checker answers a chunk's
/// TypeScript questions, and after which others, never depends on how the threads run (a
/// checker's answers can depend on what it was asked before).
fn sweep_shared(
    input: &SweepInput<'_, '_>,
    chunks: &mut [Chunk],
    dirty: Vec<FxHashSet<NodeRef>>,
    busy: bool,
    shared: &dyn SharedTypeQueries,
    threads: usize,
) -> Vec<Swept> {
    if threads <= 1 || shared.workers() <= 1 || chunks.len() <= 1 {
        return chunks
            .iter_mut()
            .zip(dirty)
            .map(|(chunk, dirty)| sweep_chunk(input, chunk, dirty, busy, Types::Shared(shared)))
            .collect();
    }
    let workers = threads.min(shared.workers()).min(chunks.len());
    let runs = cut(&dirty, workers);
    let mut work: Vec<Mutex<Run<'_>>> = Vec::with_capacity(workers);
    let mut rest = chunks;
    let mut dirty = dirty.into_iter();
    for len in runs {
        let (run, tail) = rest.split_at_mut(len);
        rest = tail;
        work.push(Mutex::new(Run {
            chunks: run,
            dirty: dirty.by_ref().take(len).collect(),
            swept: Vec::new(),
        }));
    }
    // SAFETY: the world, the links and the sweep's tables are only read while the workers
    // run; each worker writes only to its own chunks (behind `work`'s mutexes).
    let input = unsafe { Shared::new(input) };
    shared.run_workers(&|worker, queries| {
        let Some(run) = work.get(worker) else {
            return;
        };
        let input = input.get();
        let mut run = run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Run {
            chunks,
            dirty,
            swept,
        } = &mut *run;
        for (chunk, dirty) in chunks.iter_mut().zip(std::mem::take(dirty)) {
            swept.push(sweep_chunk(
                input,
                chunk,
                dirty,
                busy,
                Types::Worker(&mut *queries),
            ));
        }
    });
    work.into_iter()
        .flat_map(|run| {
            run.into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .swept
        })
        .collect()
}

/// One worker's chunks in a sweep, and what sweeping them did.
struct Run<'c> {
    chunks: &'c mut [Chunk],
    dirty: Vec<FxHashSet<NodeRef>>,
    swept: Vec<Swept>,
}

/// The lengths of `workers` contiguous runs of chunks with about as many dirty units each
/// (every chunk counts one more, for the cost of looking at it).
fn cut(dirty: &[FxHashSet<NodeRef>], workers: usize) -> Vec<usize> {
    let weight = |dirty: &FxHashSet<NodeRef>| dirty.len() + 1;
    let total: usize = dirty.iter().map(weight).sum();
    let mut runs = Vec::with_capacity(workers);
    let (mut start, mut sum) = (0, 0);
    for (at, chunk) in dirty.iter().enumerate() {
        sum += weight(chunk);
        let later_runs = workers - runs.len() - 1;
        let left = dirty.len() - at - 1;
        // Cut once this run has its share, or when each later run needs what is left.
        if later_runs > 0 && (left == later_runs || sum * workers >= total * (runs.len() + 1)) {
            runs.push(at + 1 - start);
            start = at + 1;
        }
    }
    runs.push(dirty.len() - start);
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirty(sizes: &[usize]) -> Vec<FxHashSet<NodeRef>> {
        let mut next = 0;
        sizes
            .iter()
            .map(|&size| {
                (0..size)
                    .map(|_| {
                        next += 1;
                        NodeRef {
                            module: trioxide_common::ModuleId::new(0),
                            node: oxc_syntax::node::NodeId::from_usize(next),
                        }
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn runs_share_the_dirty_units() {
        assert_eq!(cut(&dirty(&[3, 3, 3, 3]), 2), [2, 2]);
        assert_eq!(cut(&dirty(&[9, 0, 0, 1, 1, 1]), 2), [1, 5]);
        // Every run gets a chunk, however the units fall.
        assert_eq!(cut(&dirty(&[0, 0, 0, 20]), 4), [1, 1, 1, 1]);
        assert_eq!(cut(&dirty(&[20, 0, 0, 0]), 4), [1, 1, 1, 1]);
        assert_eq!(cut(&dirty(&[1, 1, 1]), 1), [3]);
    }
}
