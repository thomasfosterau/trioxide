//! Serialises [`Span`]s as `{ "start": u32, "end": u32 }`, independent of oxc's own serde.

use oxc_span::Span;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Serialize, Deserialize)]
struct Raw {
    start: u32,
    end: u32,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's `with` passes a reference"
)]
pub fn serialize<S: Serializer>(span: &Span, serializer: S) -> Result<S::Ok, S::Error> {
    Raw {
        start: span.start,
        end: span.end,
    }
    .serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Span, D::Error> {
    let raw = Raw::deserialize(deserializer)?;
    Ok(Span::new(raw.start, raw.end))
}

/// The same for `Option<Span>`.
pub mod option {
    use oxc_span::Span;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::Raw;

    #[expect(clippy::ref_option, reason = "serde's `with` passes `&Option<T>`")]
    pub fn serialize<S: Serializer>(span: &Option<Span>, serializer: S) -> Result<S::Ok, S::Error> {
        span.map(|span| Raw {
            start: span.start,
            end: span.end,
        })
        .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Span>, D::Error> {
        Ok(Option::<Raw>::deserialize(deserializer)?.map(|raw| Span::new(raw.start, raw.end)))
    }
}
