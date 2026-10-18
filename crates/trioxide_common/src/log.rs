//! The decision log: what trioxide decided for each function and pass, and why.
//!
//! Every stage appends to a per-module [`ModuleLog`] while it runs. The facade collects them
//! into its `Report`, which `trioxide explain` and `--emit=report` print (PLAN §2 step 8 and
//! §5 "Explainability"). Entries are plain data and serialise to JSON.

use oxc_span::Span;
use serde::{Deserialize, Serialize};

/// Where a function's code ends up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// Emitted as (optimised) JavaScript.
    Js,
    /// Compiled into a WASM island; the JS fallback is still produced (PLAN D5).
    Wasm,
}

/// The partitioner's verdict on one function.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionDecision {
    /// The function's name, or `None` for anonymous functions.
    pub name: Option<String>,
    /// The function's span in its module.
    #[serde(with = "crate::serde_span")]
    pub span: Span,
    /// The verdict.
    pub target: Target,
    /// Why, in one sentence. For an ineligible function this names the failed rule.
    pub reason: String,
    /// Where the failed eligibility rule was hit, if it applies to a sub-expression.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_span::option"
    )]
    pub culprit: Option<Span>,
    /// The island the function joined, for [`Target::Wasm`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub island: Option<String>,
    /// Cost-model numbers for the function's island, if it was scored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostEstimate>,
    /// The target a directive (`"use wasm"`, `"use js"`) or config override asked for, if
    /// one did: `Wasm` is "forced", `Js` "forbidden".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directive: Option<Target>,
    /// Every eligibility rule the function failed, for an ineligible function (the first one
    /// is in `reason`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<RuleFailure>,
}

/// One failed eligibility rule (PLAN §5 "Eligibility").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleFailure {
    /// What is wrong, as a sentence fragment ("calls `console.log`, a host API").
    pub message: String,
    /// The offending construct.
    #[serde(with = "crate::serde_span")]
    pub span: Span,
    /// The function it is in, when that is not the function the decision is about (a
    /// callee).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// The module it is in, when that is not the decision's module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<std::path::PathBuf>,
}

/// The cost model's numbers for one island (PLAN §5 "Cost model").
///
/// Units are estimated nanoseconds per run of the program, with every loop assumed to run
/// its estimated trip count. `cost` is the sum of the three cost terms.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CostEstimate {
    /// Estimated time WASM saves on the island's compute (negative when it is slower).
    pub benefit: f64,
    /// Boundary crossings, marshalling and instantiation.
    pub cost: f64,
    /// The threshold `benefit - cost` had to exceed.
    pub threshold: f64,
    /// The cost of the boundary crossings: calls into the island, weighted by the loops
    /// around their call sites.
    pub crossings: f64,
    /// The cost of copying arguments and results through linear memory.
    pub marshalling: f64,
    /// The fixed cost of instantiating the island's module.
    pub instantiation: f64,
    /// Estimated calls into the island per run.
    pub calls: f64,
    /// Estimated operations the island runs per run of the program.
    pub work: f64,
    /// Estimated bytes copied in and out per run.
    pub bytes: f64,
}

/// A cost-model number for humans: whole units, or scientific notation from 10¹² on (a
/// threshold of `-1e300` stays readable).
pub fn number(value: f64) -> String {
    if value.is_finite() && value.abs() < 1e12 {
        format!("{value:.0}")
    } else {
        format!("{value:.2e}")
    }
}

impl CostEstimate {
    /// `benefit - cost`, the margin compared with the threshold.
    pub fn net(&self) -> f64 {
        self.benefit - self.cost
    }

    /// Whether the island is worth it.
    pub fn profitable(&self) -> bool {
        self.net() > self.threshold
    }
}

/// What happened when a pass considered a site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PassOutcome {
    /// The pass rewrote the code.
    Applied,
    /// The pass declined, e.g. because a fact was only `Declared` and `trustTypes` is off.
    Skipped {
        /// Why it declined.
        reason: String,
    },
}

/// One pass decision at one site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassEvent {
    /// The pass's name, as used in `Options::passes`.
    pub pass: String,
    /// The site, in the module's original source.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_span::option"
    )]
    pub span: Option<Span>,
    /// What happened.
    #[serde(flatten)]
    pub outcome: PassOutcome,
    /// Extra detail for humans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Everything recorded about one module during a build or transform.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleLog {
    /// Partitioner verdicts, one per function that was considered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<FunctionDecision>,
    /// Pass decisions in the order they happened.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passes: Vec<PassEvent>,
}

impl ModuleLog {
    /// Records that `pass` rewrote the code at `span`.
    pub fn applied(&mut self, pass: &str, span: Option<Span>) {
        self.passes.push(PassEvent {
            pass: pass.to_string(),
            span,
            outcome: PassOutcome::Applied,
            message: None,
        });
    }

    /// Records that `pass` declined at `span`, and why.
    pub fn skipped(&mut self, pass: &str, span: Option<Span>, reason: impl Into<String>) {
        self.passes.push(PassEvent {
            pass: pass.to_string(),
            span,
            outcome: PassOutcome::Skipped {
                reason: reason.into(),
            },
            message: None,
        });
    }
}
