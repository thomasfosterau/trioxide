//! The whole-program backstop ("no fixed point, no facts") is a last resort: no program the
//! tests compile reaches it.

mod common;

#[test]
fn no_program_in_the_corpus_hits_the_backstop() {
    let corpus = common::corpus();
    assert!(corpus.len() > 100, "the corpus is missing: {corpus:?}");
    let mut failures = Vec::new();
    for entry in &corpus {
        let loaded = common::load(entry);
        if loaded
            .analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("did not reach a fixed point"))
        {
            let rounds = loaded.analysis.stats.rounds;
            failures.push(format!("{} ({rounds} rounds)", entry.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "no fixed point for:\n{}",
        failures.join("\n")
    );
}
