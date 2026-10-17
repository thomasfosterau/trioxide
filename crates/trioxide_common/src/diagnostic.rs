use std::{fmt, path::PathBuf};

use oxc_diagnostics::{OxcDiagnostic, Severity as OxcSeverity};
use oxc_span::Span;
use serde::{Deserialize, Serialize};

/// How serious a [`Diagnostic`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational, e.g. "this function stayed JS because …".
    Advice,
    /// Something the user should look at; output is still produced.
    Warning,
    /// The module (or build) could not be compiled.
    Error,
}

/// A source range with an optional message, pointing into the diagnostic's file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    /// Byte offsets into the file's UTF-8 source.
    #[serde(with = "crate::serde_span")]
    pub span: Span,
    /// Text shown next to the range, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// An error, warning or note about the user's code or trioxide's handling of it.
///
/// Diagnostics are plain data so they can cross the napi boundary and be written into
/// reports. Spans are UTF-8 byte offsets into `file`'s source as trioxide read it. Render them
/// for humans with [`Diagnostic::to_oxc`] and `OxcDiagnostic::render_with_source_code`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// How serious it is.
    pub severity: Severity,
    /// A stable machine-readable code such as `trioxide(unresolved-import)` or `parse`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The headline.
    pub message: String,
    /// The file the labels point into, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// Source ranges, the first one being the primary location.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<Label>,
    /// A hint on how to fix it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

impl Diagnostic {
    /// A new diagnostic with no file, labels, code or help.
    pub fn new(severity: Severity, message: impl Into<String>) -> Self {
        Self {
            severity,
            code: None,
            message: message.into(),
            file: None,
            labels: Vec::new(),
            help: None,
        }
    }

    /// An error with `message`.
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(Severity::Error, message)
    }

    /// A warning with `message`.
    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, message)
    }

    /// Sets the code.
    #[must_use]
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Sets the file the labels point into.
    #[must_use]
    pub fn with_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// Adds a label.
    #[must_use]
    pub fn with_label(mut self, span: Span, message: Option<String>) -> Self {
        self.labels.push(Label { span, message });
        self
    }

    /// Sets the help text.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Whether this is an error.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Converts an oxc diagnostic (parser, semantic or transformer) about `file`.
    pub fn from_oxc(diagnostic: &OxcDiagnostic, file: Option<PathBuf>) -> Self {
        let severity = match diagnostic.severity {
            OxcSeverity::Error => Severity::Error,
            OxcSeverity::Warning => Severity::Warning,
            OxcSeverity::Advice => Severity::Advice,
        };
        let labels = diagnostic
            .labels
            .iter()
            .map(|label| Label {
                span: Span::new(label.offset(), label.offset() + label.len()),
                message: label.label().map(str::to_string),
            })
            .collect();
        let code = (diagnostic.code.scope.is_some() || diagnostic.code.number.is_some())
            .then(|| diagnostic.code.to_string());
        Self {
            severity,
            code,
            message: diagnostic.message.to_string(),
            file,
            labels,
            help: diagnostic.help.as_ref().map(ToString::to_string),
        }
    }

    /// Converts into an oxc diagnostic for graphical rendering, e.g. with
    /// `render_with_source_code`.
    pub fn to_oxc(&self) -> OxcDiagnostic {
        let mut diagnostic = match self.severity {
            Severity::Error => OxcDiagnostic::error(self.message.clone()),
            Severity::Warning => OxcDiagnostic::warn(self.message.clone()),
            Severity::Advice => {
                OxcDiagnostic::warn(self.message.clone()).with_severity(OxcSeverity::Advice)
            }
        };
        if !self.labels.is_empty() {
            diagnostic =
                diagnostic.with_labels(self.labels.iter().map(|label| match &label.message {
                    Some(message) => label.span.label(message.clone()),
                    None => label.span.into(),
                }));
        }
        if let Some(help) = &self.help {
            diagnostic = diagnostic.with_help(help.clone());
        }
        diagnostic
    }
}

impl fmt::Display for Diagnostic {
    /// A one-line rendering: `path:start: severity[code]: message`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{}", file.display())?;
            if let Some(label) = self.labels.first() {
                write!(f, ":{}", label.span.start)?;
            }
            f.write_str(": ")?;
        }
        let severity = match self.severity {
            Severity::Advice => "advice",
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        f.write_str(severity)?;
        if let Some(code) = &self.code {
            write!(f, "[{code}]")?;
        }
        write!(f, ": {}", self.message)
    }
}
