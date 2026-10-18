//! The tsrs oracle's queries, span matching, diagnostics and program lifecycle.

#![cfg(feature = "tsrs")]

use std::path::{Path, PathBuf};

use trioxide_common::{ModuleId, NodeId, NodeRef, Severity, Span};
use trioxide_types::{
    Interval, NumberType, OracleInput, RebuildPolicy, Shape, Signature, SourceText, TsrsOracle,
    Type, TypeOracle, TypedArrayKind,
};

const ROOT: &str = "/__trioxide_tests__/project";

/// An oracle over in-memory modules `(file name, source)`, rooted at a directory that does
/// not exist on disk.
struct Fixture {
    oracle: TsrsOracle,
    files: Vec<(PathBuf, String)>,
}

impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        Self::with(files, RebuildPolicy::default(), false, None)
    }

    fn with(
        files: &[(&str, &str)],
        policy: RebuildPolicy,
        check: bool,
        tsconfig: Option<&Path>,
    ) -> Self {
        let files: Vec<(PathBuf, String)> = files
            .iter()
            .map(|(name, source)| (Path::new(ROOT).join(name), (*source).to_string()))
            .collect();
        let mut oracle = TsrsOracle::new(policy);
        let root = tsconfig.map_or_else(
            || PathBuf::from(ROOT),
            |path| path.parent().unwrap().to_path_buf(),
        );
        let modules: Vec<SourceText<'_>> = files
            .iter()
            .enumerate()
            .map(|(index, (path, source))| SourceText {
                module: module(index),
                path,
                source,
            })
            .collect();
        let diagnostics = oracle.load(&OracleInput {
            root: &root,
            tsconfig,
            modules: &modules,
            check,
        });
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        Self { oracle, files }
    }

    /// The span of the `nth` (0-based) occurrence of `needle` in file `index`.
    fn span(&self, index: usize, needle: &str, nth: usize) -> Span {
        let source = &self.files[index].1;
        let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
        let word = needle.starts_with(ident) && needle.ends_with(ident);
        let start = source
            .match_indices(needle)
            .filter(|&(at, _)| {
                !word
                    || (!source[..at].ends_with(ident)
                        && !source[at + needle.len()..].starts_with(ident))
            })
            .nth(nth)
            .unwrap_or_else(|| panic!("`{needle}` #{nth} not in file {index}"))
            .0;
        let start = u32::try_from(start).unwrap();
        Span::new(start, start + u32::try_from(needle.len()).unwrap())
    }

    fn type_of(&mut self, needle: &str, nth: usize) -> Option<Type> {
        let span = self.span(0, needle, nth);
        self.oracle.type_at(module(0), span)
    }

    fn update(&mut self, index: usize, source: &str) {
        self.files[index].1 = source.to_string();
        self.oracle.update(SourceText {
            module: module(index),
            path: &self.files[index].0,
            source,
        });
    }
}

fn module(index: usize) -> ModuleId {
    ModuleId::new(u32::try_from(index).unwrap())
}

fn number_constant(value: f64) -> Type {
    Type::Number(NumberType::constant(value))
}

#[test]
fn primitives_literals_and_unions() {
    let mut f = Fixture::new(&[(
        "main.ts",
        "const one = 1;\nlet n = 2;\nconst s = `a${n}`;\nconst b = n > 1;\nconst big = 10n;\n\
         declare const maybe: number | undefined;\nconst u = maybe;\nconst a: any = 1;\n\
         declare function v(): void;\nconst r = v();\nconst small: 1 | 2 | 3 = 2;\nconst x = null;\n",
    )]);
    assert_eq!(f.type_of("one", 0), Some(number_constant(1.0)));
    assert_eq!(f.type_of("n", 0), Some(Type::NUMBER));
    assert_eq!(f.type_of("s", 0), Some(Type::String));
    assert_eq!(f.type_of("b", 0), Some(Type::Boolean));
    assert_eq!(f.type_of("big", 0), Some(Type::BigInt));
    assert_eq!(
        f.type_of("u", 0),
        Some(Type::Union(std::sync::Arc::from([
            Type::Undefined,
            Type::NUMBER
        ])))
    );
    // `any` and `void` say nothing about the value.
    assert_eq!(f.type_of("a", 0), Some(Type::Unknown));
    assert_eq!(f.type_of("r", 0), Some(Type::Unknown));
    assert_eq!(
        f.type_of("small", 0),
        Some(Type::Number(NumberType {
            range: Interval::new(1.0, 3.0),
            int: true,
            non_nan: true,
            non_neg_zero: true,
        }))
    );
    assert_eq!(f.type_of("x", 0), Some(Type::Null));
    let flags = f
        .oracle
        .type_flags_at(module(0), f.span(0, "a", 0))
        .unwrap();
    assert!(flags.any);
    let flags = f
        .oracle
        .type_flags_at(module(0), f.span(0, "u", 0))
        .unwrap();
    assert!(flags.nullable && !flags.any);
}

#[test]
fn arrays_tuples_typed_arrays_and_objects() {
    let mut f = Fixture::new(&[(
        "main.ts",
        "const xs = [1, 2, 3];\ndeclare const ro: readonly string[];\n\
         const pair: [number, string] = [1, 'a'];\ndeclare const opt: [number, string?];\n\
         const fa = new Float64Array(4);\nconst u8 = new Uint8ClampedArray(2);\n\
         const p = { x: 1, y: 'b' };\ndeclare const m: Map<string, number>;\n\
         declare const rec: Record<string, number>;\n",
    )]);
    assert_eq!(
        f.type_of("xs", 0),
        Some(Type::Array(std::sync::Arc::new(Type::NUMBER)))
    );
    assert_eq!(
        f.type_of("ro", 0),
        Some(Type::Array(std::sync::Arc::new(Type::String)))
    );
    assert_eq!(
        f.type_of("pair", 0),
        Some(Type::Tuple(std::sync::Arc::from([
            Type::NUMBER,
            Type::String
        ])))
    );
    assert_eq!(
        f.type_of("opt", 0),
        Some(Type::Array(std::sync::Arc::new(Type::Union(
            std::sync::Arc::from([Type::Undefined, Type::NUMBER, Type::String,])
        ))))
    );
    assert_eq!(
        f.type_of("fa", 0),
        Some(Type::TypedArray(TypedArrayKind::Float64))
    );
    assert_eq!(
        f.type_of("u8", 0),
        Some(Type::TypedArray(TypedArrayKind::Uint8Clamped))
    );
    assert_eq!(
        f.type_of("p", 0),
        Some(Type::Object(std::sync::Arc::new(Shape {
            properties: vec![("x".into(), Type::NUMBER), ("y".into(), Type::String)],
            exact: false,
        })))
    );
    // Library classes and index signatures have no faithful trioxide type.
    assert_eq!(f.type_of("m", 0), Some(Type::Unknown));
    assert_eq!(f.type_of("rec", 0), Some(Type::Unknown));
}

#[test]
fn classes_and_functions_name_registered_declarations() {
    let source = "class Point { constructor(public x: number) {} }\nconst p = new Point(1);\n\
                  function twice(n: number): number { return n * 2; }\nconst t = twice;\n\
                  class Other {}\nconst o = new Other();\n";
    let mut f = Fixture::new(&[("main.ts", source)]);
    let point = NodeId::from_usize(7);
    let twice = NodeId::from_usize(9);
    let class_name = f.span(0, "Point", 0);
    let function_name = f.span(0, "twice", 0);
    f.oracle
        .register_declarations(module(0), &[(class_name, point), (function_name, twice)]);
    assert_eq!(
        f.type_of("p", 0),
        Some(Type::Instance(NodeRef {
            module: module(0),
            node: point,
        }))
    );
    assert_eq!(
        f.type_of("t", 0),
        Some(Type::Function(
            std::sync::Arc::new(Signature {
                params: vec![Type::NUMBER],
                rest: false,
                ret: Type::NUMBER,
            }),
            Some(NodeRef {
                module: module(0),
                node: twice,
            })
        ))
    );
    // Not registered: the oracle can't name it, so it says nothing.
    assert_eq!(f.type_of("o", 0), Some(Type::Unknown));
}

#[test]
fn spans_through_parentheses_exports_and_jsdoc() {
    let source = "/** Doc. */\nexport function f(a: number) {\n  return ((a + 1));\n}\n\
                  export const g = (/* c */ a: number) => a;\n";
    let mut f = Fixture::new(&[("main.ts", source)]);
    let m = module(0);
    // Every parenthesisation level is its own node.
    for needle in ["a + 1", "(a + 1)", "((a + 1))"] {
        assert_eq!(f.type_of(needle, 0), Some(Type::NUMBER), "{needle}");
    }
    // oxc's declaration starts after `export`; tsrs's includes it.
    let declaration = Span::new(f.span(0, "function", 0).start, f.span(0, "}", 0).end);
    assert!(f.oracle.matches_node(m, declaration));
    // Spans that start inside trivia, or cover no single node, match nothing.
    assert!(!f.oracle.matches_node(m, f.span(0, "Doc", 0)));
    assert!(!f.oracle.matches_node(m, f.span(0, "f(a", 0)));
    assert_eq!(f.oracle.type_at(m, f.span(0, "c */", 0)), None);
}

#[test]
fn jsdoc_types_in_javascript() {
    let source = "/** @type {number} */\nlet count = JSON.parse('1');\n/** @param {string} s */\nfunction up(s) { return s; }\n";
    let mut f = Fixture::new(&[("main.js", source)]);
    assert_eq!(f.type_of("count", 0), Some(Type::NUMBER));
    let s = f.span(0, "s", 2);
    assert_eq!(f.oracle.type_at(module(0), s), Some(Type::String));
    // The JSDoc type itself is not an oxc node.
    assert!(!f.oracle.matches_node(module(0), f.span(0, "number", 0)));
}

#[test]
fn declarations_symbols_and_signatures() {
    let mut f = Fixture::new(&[
        (
            "main.ts",
            "import { over, value } from './lib.js';\nexport const a = over('s');\nexport const b = over(1);\n\
             let v: string | number = value;\nif (typeof v === 'string') { v; }\n",
        ),
        (
            "lib.ts",
            "export function over(x: string): string;\nexport function over(x: number): number;\n\
             export function over(x: unknown) { return x; }\nexport const value: string | number = 1;\n",
        ),
    ]);
    let m = module(0);
    let call = f.span(0, "over('s')", 0);
    assert_eq!(
        f.oracle.resolved_signature(m, call),
        Some(Signature {
            params: vec![Type::String],
            rest: false,
            ret: Type::String,
        })
    );
    assert_eq!(
        f.oracle.type_at(m, f.span(0, "over(1)", 0)),
        Some(Type::NUMBER)
    );
    // Imports resolve to the exporting module's declaration name.
    let lib_value = f.span(1, "value", 0);
    assert_eq!(
        f.oracle.declaration_of(m, f.span(0, "value", 1)),
        Some((module(1), lib_value))
    );
    // Flow narrowing applies to the expression, not to the symbol.
    let narrowed = f.span(0, "v;", 0);
    let narrowed = Span::new(narrowed.start, narrowed.start + 1);
    assert_eq!(f.oracle.type_at(m, narrowed), Some(Type::String));
    assert_eq!(
        f.oracle.symbol_type_at(m, narrowed),
        Some(Type::Union(std::sync::Arc::from([
            Type::NUMBER,
            Type::String
        ])))
    );
    assert_eq!(
        f.oracle
            .type_string_at(m, f.span(0, "over('s')", 0))
            .as_deref(),
        Some("string")
    );
}

#[test]
fn check_reports_typescript_errors() {
    let source = "const n: number = 'no';\nexport {};\n";
    let mut f = Fixture::with(&[("main.ts", source)], RebuildPolicy::default(), true, None);
    let diagnostics = f.oracle.diagnostics(module(0));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let error = &diagnostics[0];
    assert_eq!(error.severity, Severity::Error);
    assert_eq!(error.code.as_deref(), Some("TS2322"));
    assert!(
        error.message.contains("not assignable"),
        "{}",
        error.message
    );
    assert_eq!(error.labels[0].span, f.span(0, "n", 0));
    assert_eq!(error.file.as_deref(), Some(f.files[0].0.as_path()));

    // Without `check`, nothing is reported.
    let mut quiet = Fixture::new(&[("main.ts", source)]);
    assert_eq!(quiet.oracle.diagnostics(module(0)), Vec::new());
}

#[test]
fn reads_the_projects_tsconfig() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let tsconfig = dir.join("tsconfig.json");
    std::fs::write(
        &tsconfig,
        r#"{ "compilerOptions": { "strict": false, "noEmit": true }, "include": [] }"#,
    )
    .unwrap();
    let source = "let x: number = null;\nexport {};\n";
    let path = dir.join("main.ts");
    let mut oracle = TsrsOracle::default();
    let diagnostics = oracle.load(&OracleInput {
        root: &dir,
        tsconfig: Some(&tsconfig),
        modules: &[SourceText {
            module: module(0),
            path: &path,
            source,
        }],
        check: true,
    });
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    // Without strictNullChecks, assigning null is fine.
    assert_eq!(oracle.diagnostics(module(0)), Vec::new());
}

#[test]
fn updates_are_incremental_until_the_policy_says_rebuild() {
    let policy = RebuildPolicy {
        max_updates: 2,
        max_resident_bytes: None,
    };
    let mut f = Fixture::with(
        &[
            ("main.ts", "export const x = 1;\n"),
            ("other.ts", "export const y = 'a';\n"),
        ],
        policy,
        false,
        None,
    );
    assert_eq!(f.type_of("x", 0), Some(number_constant(1.0)));
    for (round, value) in [("1", "'s'"), ("2", "true"), ("3", "[1]")] {
        f.update(0, &format!("export const x = {value};\n"));
        let expected = match round {
            "1" => Type::String,
            "2" => Type::Boolean,
            _ => Type::Array(std::sync::Arc::new(Type::NUMBER)),
        };
        assert_eq!(f.type_of("x", 0), Some(expected), "round {round}");
    }
    let stats = f.oracle.program_stats();
    assert_eq!(stats.updates, 2, "{stats:?}");
    assert_eq!(stats.full_builds, 2, "{stats:?}");
    assert_eq!(stats.panics, 0);
    // The untouched module still answers.
    let y = f.span(1, "y", 0);
    assert_eq!(f.oracle.type_at(module(1), y), Some(Type::String));
}

#[test]
fn unknown_modules_and_spans_answer_none() {
    let mut f = Fixture::new(&[("main.ts", "const a = 1;\n")]);
    assert_eq!(f.oracle.type_at(module(9), Span::new(0, 1)), None);
    assert_eq!(f.oracle.type_at(module(0), Span::new(0, 100)), None);
    assert_eq!(f.oracle.declaration_of(module(0), Span::new(0, 5)), None);
    let stats = f.oracle.span_stats();
    assert_eq!(stats.matched, 0);
    assert_eq!(stats.unmatched, 2);
}

#[test]
fn the_shared_view_answers_like_the_oracle_from_many_threads() {
    let files: Vec<(String, String)> = (0..12)
        .map(|index| {
            (
                format!("m{index}.ts"),
                format!(
                    "export class C{index} {{ x = {index}; }}\n\
                     export const c = new C{index}();\n\
                     export const n = c.x + {index};\n\
                     export const xs = [c.x, n];\n\
                     export function f(v: number): string {{ return String(v + n); }}\n\
                     export const s = f(n);\n"
                ),
            )
        })
        .collect();
    let named: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, source)| (name.as_str(), source.as_str()))
        .collect();
    let mut f = Fixture::new(&named);
    let mut queries = Vec::new();
    for index in 0..files.len() {
        let class = f.span(index, &format!("C{index}"), 0);
        f.oracle
            .register_declarations(module(index), &[(class, NodeId::from_usize(index))]);
        for needle in ["c", "n", "xs", "f", "s", "c.x"] {
            queries.push((module(index), f.span(index, needle, 0)));
        }
    }
    let expected: Vec<Option<Type>> = queries
        .iter()
        .map(|&(module, span)| f.oracle.type_at(module, span))
        .collect();
    assert!(expected.iter().all(Option::is_some));
    let shared = f
        .oracle
        .shared()
        .expect("tsrs answers from several threads");
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|offset| {
                let queries = &queries;
                scope.spawn(move || {
                    // Each thread walks the queries in a different order.
                    (0..queries.len())
                        .map(|step| (step * 5 + offset) % queries.len())
                        .map(|at| (at, shared.type_at(queries[at].0, queries[at].1)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            for (at, answer) in handle.join().unwrap() {
                assert_eq!(answer, expected[at], "query {at}");
            }
        }
    });
    // Workers with a checker each: every worker answers every query the same way.
    assert!(shared.workers() >= 1);
    let answers = std::sync::Mutex::new(Vec::new());
    shared.run_workers(&|worker, checker| {
        for (at, &(module, span)) in queries.iter().enumerate() {
            let answer = checker.type_at(module, span);
            answers.lock().unwrap().push((worker, at, answer));
        }
    });
    let answers = answers.into_inner().unwrap();
    assert_eq!(answers.len(), queries.len() * shared.workers());
    for (worker, at, answer) in answers {
        assert_eq!(answer, expected[at], "worker {worker}, query {at}");
    }
}
