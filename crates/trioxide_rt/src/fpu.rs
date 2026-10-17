//! Square root and rounding to integers: hardware instructions with the `std` feature,
//! exact software versions without it. Both give identical results.

#[cfg(feature = "std")]
mod imp {
    pub fn sqrt(x: f64) -> f64 {
        x.sqrt()
    }

    pub fn trunc(x: f64) -> f64 {
        x.trunc()
    }

    pub fn floor(x: f64) -> f64 {
        x.floor()
    }

    pub fn ceil(x: f64) -> f64 {
        x.ceil()
    }
}

#[cfg(not(feature = "std"))]
mod imp {
    pub use super::soft::{ceil, floor, trunc};

    pub fn sqrt(x: f64) -> f64 {
        libm::sqrt(x)
    }
}

pub use imp::{ceil, floor, sqrt, trunc};

/// Rounding through the saturating float→int conversion (`i64.trunc_sat_f64_s` on wasm32),
/// which is exact for every |x| < 2^52; larger values are already integers.
#[cfg_attr(feature = "std", cfg(test))]
pub mod soft {
    const TWO_POW_52: f64 = 4_503_599_627_370_496.0;

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "|x| < 2^52 fits i64 and converts back exactly"
    )]
    pub fn trunc(x: f64) -> f64 {
        if x.abs() < TWO_POW_52 {
            (x as i64 as f64).copysign(x)
        } else {
            x
        }
    }

    pub fn floor(x: f64) -> f64 {
        let t = trunc(x);
        if t > x { t - 1.0 } else { t }
    }

    pub fn ceil(x: f64) -> f64 {
        let t = trunc(x);
        if t < x { t + 1.0 } else { t }
    }
}

#[cfg(test)]
mod tests {
    use super::soft;

    #[test]
    fn soft_rounding_matches_libm() {
        let mut values = vec![
            0.0,
            -0.0,
            0.5,
            -0.5,
            1.5,
            -1.5,
            0.499_999_999_999_999_94,
            -0.499_999_999_999_999_94,
            4_503_599_627_370_495.5,
            -4_503_599_627_370_495.5,
            4_503_599_627_370_496.0,
            9_007_199_254_740_993.0,
            1e300,
            -1e300,
            f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE,
            5e-324,
            -5e-324,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ];
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..10_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            values.push(f64::from_bits(state));
        }
        for x in values {
            for (name, ours, theirs) in [
                ("trunc", soft::trunc(x), libm::trunc(x)),
                ("floor", soft::floor(x), libm::floor(x)),
                ("ceil", soft::ceil(x), libm::ceil(x)),
            ] {
                assert!(
                    ours.to_bits() == theirs.to_bits() || (ours.is_nan() && theirs.is_nan()),
                    "{name}({x:e}): {ours:e} != {theirs:e}"
                );
            }
        }
    }
}
