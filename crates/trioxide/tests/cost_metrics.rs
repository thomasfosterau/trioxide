//! The cost model's operation counts against what the Rust backend generates, on whole
//! programs (S9 integration tests).

use trioxide::{CostModel, Options, Project, log::FunctionDecision};

/// The image-filters calibration kernel: `boxBlur`'s dimensions arrive as parameters, so the
/// analysis proves the read index `(ny * width + nx) * 4 + c` an int32 but not `ny * width`,
/// and the backend computes that index in `f64` and checks every read.
const KERNEL: &str = include_str!("../../../benchmarks/partition/kernels/image-filters/kernel.ts");
const INPUT: &str = include_str!("../../../benchmarks/partition/kernels/image-filters/input.ts");

/// The cost-model decision on `function` in a program of `kernel.ts` and `input.ts` (the
/// entry), planned with only `weights` non-zero among the operation weights, so its benefit
/// counts those operations.
fn decision(
    kernel: &str,
    input: &str,
    function: &str,
    weights: impl FnOnce(&mut CostModel),
) -> FunctionDecision {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("kernel.ts"), kernel).unwrap();
    std::fs::write(dir.path().join("input.ts"), input).unwrap();
    let mut cost_model = CostModel {
        arithmetic: 0.0,
        integer: 0.0,
        conversion: 0.0,
        wrapping: 0.0,
        math: 0.0,
        remainder: 0.0,
        index: 0.0,
        checked_index: 0.0,
        iteration: 0.0,
        ..CostModel::default()
    };
    weights(&mut cost_model);
    let options = Options {
        cost_model,
        ..Options::default()
    };
    let mut project = Project::new(dir.path().to_path_buf(), options).unwrap();
    let report = project.plan(&[dir.path().join("input.ts")]).unwrap();
    report
        .modules
        .iter()
        .flat_map(|m| &m.log.functions)
        .find(|f| f.name.as_deref() == Some(function))
        .cloned()
        .unwrap()
}

fn box_blur(weights: impl FnOnce(&mut CostModel)) -> FunctionDecision {
    decision(KERNEL, INPUT, "boxBlur", weights)
}

fn benefit(decision: &FunctionDecision) -> f64 {
    decision
        .cost
        .as_ref()
        .expect("the function is scored")
        .benefit
}

/// An index whose sum is an int32 but whose product isn't proven one is float arithmetic in
/// the island, so its accesses count as checked (they used to count as int32-indexed, which
/// made the model expect WASM to be faster where it is 3–4× slower).
#[test]
fn index_arithmetic_counts_as_int32_only_when_every_operation_is() {
    // 9 neighbours × 3 channels × 320×240 pixels, read with a float index.
    let checked = box_blur(|m| m.checked_index = 1.0);
    assert_eq!(benefit(&checked), 2_073_600.0, "{checked:?}");
    // Four writes and the alpha read per pixel index with `(y * width + x) * 4 + c`: int32.
    let int32 = box_blur(|m| m.index = 1.0);
    assert_eq!(benefit(&int32), 5.0 * 76_800.0, "{int32:?}");
}

/// `%` on int32s is `f64` in the island (`RemI32` has an `f64` result), so an index built on
/// it is a float index, whatever its range.
#[test]
fn remainders_make_float_indices() {
    let kernel = "export function pick(a: Int32Array): number {
  let s = 0;
  for (let i = 0; i < 400000; i++) s += a[(i % 7) * 4 + 1];
  return s;
}
";
    let input = "import { pick } from \"./kernel.js\";
const a = new Int32Array(64);
export const run = (): number => pick(a);
";
    let checked = decision(kernel, input, "pick", |m| m.checked_index = 1.0);
    assert_eq!(benefit(&checked), 400_000.0, "{checked:?}");
    let int32 = decision(kernel, input, "pick", |m| m.index = 1.0);
    assert_eq!(benefit(&int32), 0.0, "{int32:?}");
}
