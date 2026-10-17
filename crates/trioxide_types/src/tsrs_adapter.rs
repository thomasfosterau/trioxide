//! The tsrs side of the adapter. Nothing in this module may leak a tsrs type through the
//! crate's public API: callers get trioxide types, strings or spans only.
//!
//! For now this only holds the in-memory program setup that the smoke test exercises. S2 grows
//! it into the real [`TypeOracle`](crate::TypeOracle) implementation (span map, queries, type
//! conversion, incremental updates).

use std::sync::Arc;

use tsrs_compiler::{Context, Program, ProgramOptions, new_compiler_host, new_program};
use tsrs_core::P;
use tsrs_tsoptions::{ParseConfigHost, ParsedCommandLine, get_parsed_command_line_of_config_file};
use tsrs_vfs::{FS, bundled, vfstest};

/// The directory every in-memory program is rooted at.
const ROOT: &str = "/";

/// A tsrs program built over an in-memory file system.
///
/// tsrs programs and their arenas are `'static` and are never freed (PLAN §3, "Embedding
/// constraints"), so every `InMemoryProgram` leaks for the life of the process.
pub struct InMemoryProgram {
    program: &'static Program,
}

impl InMemoryProgram {
    /// Builds a program from `(absolute path, contents)` pairs. One of them must be
    /// `/tsconfig.json`; the bundled `lib.d.ts` files are available unless it sets `noLib`.
    ///
    /// Returns the tsconfig diagnostics as strings if the config can't be parsed.
    pub fn new(files: &[(&str, &str)]) -> Result<Self, Vec<String>> {
        let fs: Arc<dyn FS> = Arc::new(bundled::wrap_fs(vfstest::from_map(
            files.iter().copied(),
            true,
        )));
        let config = parse_config(&fs)?;
        let host = new_compiler_host(ROOT, fs, &bundled::lib_path(), None, None, None);
        Ok(Self {
            program: new_program(ProgramOptions::new(config, host)),
        })
    }

    /// The checker's rendering of the type at the token touching byte `offset` of `file`, or
    /// `None` if `file` is not part of the program.
    pub fn type_string_at(&self, file: &str, offset: u32) -> Option<String> {
        let source_file = self.program.get_source_file(file)?;
        let node = tsrs_astnav::get_touching_property_name(source_file, offset);
        let mut checker = self
            .program
            .get_type_checker_for_file(&Context::default(), source_file);
        let ty = checker.get_type_at_location(node);
        Some(checker.type_to_string_exported(ty))
    }
}

struct ConfigHost {
    fs: Arc<dyn FS>,
}

impl ParseConfigHost for ConfigHost {
    fn fs(&self) -> &dyn FS {
        &*self.fs
    }

    fn get_current_directory(&self) -> &str {
        ROOT
    }
}

fn parse_config(fs: &Arc<dyn FS>) -> Result<P<ParsedCommandLine>, Vec<String>> {
    // tsrs wants a `'static` host; like the program itself, it lives for the process.
    let host: &'static ConfigHost = Box::leak(Box::new(ConfigHost { fs: Arc::clone(fs) }));
    let (config, diagnostics) =
        get_parsed_command_line_of_config_file("/tsconfig.json", None, None, host, None);
    match config {
        Some(config) if diagnostics.is_empty() => Ok(P::new(config)),
        _ => Err(diagnostics
            .iter()
            .map(|d| d.message_text().to_string())
            .collect()),
    }
}

#[cfg(test)]
mod tests {
    use oxc_allocator::Allocator;
    use oxc_ast::ast::{Declaration, Statement};
    use oxc_parser::Parser;
    use oxc_span::{GetSpan, SourceType};

    use super::InMemoryProgram;

    /// Proves oxc and tsrs build and run in one binary: parse a module with oxc, take the span
    /// of an identifier from oxc's AST, and ask a tsrs program built from an in-memory VFS
    /// (with the bundled libs) for the type at that span.
    #[test]
    fn smoke_type_of_identifier() {
        let source = "const xs = [1, 2, 3];\nexport const total = xs.reduce((a, b) => a + b, 0);\n";

        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
        assert!(parsed.diagnostics.is_empty());
        let Statement::ExportDeclaration(export) = &parsed.program.body[1] else {
            panic!("expected an export")
        };
        let Declaration::VariableDeclaration(declaration) = &export.declaration else {
            panic!("expected a const")
        };
        let total = declaration.declarations[0].id.span();

        let program = InMemoryProgram::new(&[
            ("/tsconfig.json", r#"{ "compilerOptions": { "strict": true, "target": "es2022" }, "files": ["main.ts"] }"#),
            ("/main.ts", source),
        ])
        .expect("tsconfig parses");

        let ty = program
            .type_string_at("/main.ts", total.start)
            .expect("main.ts is in the program");
        #[expect(
            clippy::print_stderr,
            reason = "the smoke test reports what tsrs answered"
        )]
        {
            eprintln!("tsrs: type of `{}` = {ty}", total.source_text(source));
        }
        assert_eq!(ty, "number");

        let xs = u32::try_from(source.find("xs").unwrap()).unwrap();
        assert_eq!(
            program.type_string_at("/main.ts", xs).as_deref(),
            Some("number[]")
        );
    }
}
