//! Read-only cross-file Java declaration index and conservative resolver.

mod declarations;
mod imports;
mod resolver;
mod schema;

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};

use crate::java_syntax::{analyze_java_syntax, JavaSyntaxOptions};

use declarations::build_declarations;
use imports::build_file_contexts;
use resolver::JavaResolver;

pub use schema::{
    JavaCandidateOrigin, JavaIndexCounts, JavaIndexFile, JavaIndexLimits, JavaIndexProjectReport,
    JavaIndexSourceSummary, JavaIndexTimings, JavaIndexTruncation, JavaIndexedPackage,
    JavaIndexedReference, JavaIndexedSymbol, JavaIndexedSymbolKind, JavaReferenceResolution,
    JavaResolutionCandidate, JavaResolutionStatus, JavaVisibility,
};

pub const JAVA_INDEX_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_JAVA_INDEX_SYMBOL_LIMIT: usize = 100_000;
pub const MAX_JAVA_INDEX_SYMBOL_LIMIT: usize = 1_000_000;
pub const DEFAULT_JAVA_INDEX_REFERENCE_LIMIT: usize = 200_000;
pub const MAX_JAVA_INDEX_REFERENCE_LIMIT: usize = 2_000_000;
pub const DEFAULT_JAVA_INDEX_CANDIDATE_LIMIT: usize = 16;
pub const MAX_JAVA_INDEX_CANDIDATE_LIMIT: usize = 256;

#[derive(Debug, Clone, Copy)]
pub struct JavaIndexOptions {
    pub syntax: JavaSyntaxOptions,
    pub max_symbols: usize,
    pub max_references: usize,
    pub max_candidates_per_reference: usize,
}

impl Default for JavaIndexOptions {
    fn default() -> Self {
        Self {
            syntax: JavaSyntaxOptions::default(),
            max_symbols: DEFAULT_JAVA_INDEX_SYMBOL_LIMIT,
            max_references: DEFAULT_JAVA_INDEX_REFERENCE_LIMIT,
            max_candidates_per_reference: DEFAULT_JAVA_INDEX_CANDIDATE_LIMIT,
        }
    }
}

pub fn analyze_java_index(
    input: &Path,
    options: JavaIndexOptions,
) -> Result<JavaIndexProjectReport> {
    validate_options(options)?;
    let started_at = Instant::now();
    let syntax = analyze_java_syntax(input, options.syntax)?;
    let parse_us = syntax.files.iter().fold(0u64, |total, file| {
        total.saturating_add(file.parse_duration_us)
    });
    let syntax_collection_us = syntax.files.iter().fold(0u64, |total, file| {
        total.saturating_add(file.analysis_duration_us)
    });
    let discovery_and_read_us = syntax
        .duration_us
        .saturating_sub(parse_us)
        .saturating_sub(syntax_collection_us);

    let declaration_started = Instant::now();
    let declarations = build_declarations(&syntax, options.max_symbols);
    let contexts = build_file_contexts(&declarations.files);
    let declaration_index_us = duration_us(declaration_started.elapsed());

    let resolution_started = Instant::now();
    let resolver = JavaResolver::new(&declarations.symbols, options.max_candidates_per_reference);
    let retained_source_references = syntax.files.iter().fold(0usize, |total, file| {
        total.saturating_add(file.references.len())
    });
    let mut references = Vec::with_capacity(retained_source_references.min(options.max_references));
    for file in &syntax.files {
        let Some(context) = contexts.get(&file.path) else {
            continue;
        };
        for reference in &file.references {
            if references.len() >= options.max_references {
                break;
            }
            references.push(resolver.index_reference(context, reference));
        }
        if references.len() >= options.max_references {
            break;
        }
    }
    references.sort_by(|left, right| left.id.cmp(&right.id));
    let resolution_us = duration_us(resolution_started.elapsed());

    let mut counts = JavaIndexCounts {
        packages: declarations.packages.len(),
        declarations: declarations.symbols.len(),
        references: references.len(),
        ..JavaIndexCounts::default()
    };
    for reference in &references {
        match reference.resolution.status {
            JavaResolutionStatus::Exact => counts.exact += 1,
            JavaResolutionStatus::UniqueCandidate => counts.unique_candidate += 1,
            JavaResolutionStatus::Ambiguous => counts.ambiguous += 1,
            JavaResolutionStatus::Unresolved => counts.unresolved += 1,
            JavaResolutionStatus::InvalidSyntaxContext => counts.invalid_syntax_context += 1,
        }
        if reference.resolution.candidates_truncated {
            counts.candidate_lists_truncated += 1;
        }
    }

    let truncation = JavaIndexTruncation {
        source: syntax.truncated,
        symbols: declarations.symbols_truncated,
        references: retained_source_references > options.max_references,
        candidates: counts.candidate_lists_truncated > 0,
    };
    let analysis_complete = syntax.analysis_complete
        && syntax.syntax_error_files == 0
        && !truncation.source
        && !truncation.symbols
        && !truncation.references;
    let source = JavaIndexSourceSummary {
        syntax_schema_version: syntax.schema_version,
        discovered_files: syntax.discovered_files,
        selected_files: syntax.selected_files,
        parsed_files: syntax.parsed_files,
        syntax_error_files: syntax.syntax_error_files,
        skipped_large_files: syntax.skipped_large_files,
        skipped_non_utf8_files: syntax.skipped_non_utf8_files,
        skipped_linked_entries: syntax.skipped_linked_entries,
        walk_errors: syntax.walk_errors,
        read_errors: syntax.read_errors,
        source_analysis_complete: syntax.analysis_complete,
        source_truncated: syntax.truncated,
    };
    let mut warnings = syntax.warnings.clone();
    if declarations.symbols_truncated {
        warnings.push(format!(
            "Java index symbol limit reached: retained {} of at least {} declarations",
            declarations.symbols.len(),
            declarations.source_symbol_count
        ));
    }
    if retained_source_references > options.max_references {
        warnings.push(format!(
            "Java index reference limit reached: retained {} of {} parsed references",
            references.len(),
            retained_source_references
        ));
    }
    if counts.candidate_lists_truncated > 0 {
        warnings.push(format!(
            "{} reference candidate lists reached the per-reference limit",
            counts.candidate_lists_truncated
        ));
    }

    Ok(JavaIndexProjectReport {
        schema_version: JAVA_INDEX_SCHEMA_VERSION,
        operation: "java_index",
        root: syntax.root,
        input: syntax.input,
        limits: JavaIndexLimits {
            max_files: options.syntax.max_files,
            max_file_bytes: options.syntax.max_file_bytes,
            max_items_per_file_kind: options.syntax.max_items_per_kind,
            max_symbols: options.max_symbols,
            max_references: options.max_references,
            max_candidates_per_reference: options.max_candidates_per_reference,
        },
        source,
        analysis_complete,
        truncated: truncation.any(),
        truncation,
        counts,
        timings: JavaIndexTimings {
            discovery_and_read_us,
            parse_us,
            syntax_collection_us,
            declaration_index_us,
            resolution_us,
            total_us: duration_us(started_at.elapsed()),
            serialization_us: None,
        },
        packages: declarations.packages,
        files: declarations.files,
        symbols: declarations.symbols,
        references,
        warnings,
    })
}

fn validate_options(options: JavaIndexOptions) -> Result<()> {
    if options.max_symbols == 0 || options.max_symbols > MAX_JAVA_INDEX_SYMBOL_LIMIT {
        bail!(
            "Java index symbol limit must be between 1 and {}",
            MAX_JAVA_INDEX_SYMBOL_LIMIT
        );
    }
    if options.max_references == 0 || options.max_references > MAX_JAVA_INDEX_REFERENCE_LIMIT {
        bail!(
            "Java index reference limit must be between 1 and {}",
            MAX_JAVA_INDEX_REFERENCE_LIMIT
        );
    }
    if options.max_candidates_per_reference == 0
        || options.max_candidates_per_reference > MAX_JAVA_INDEX_CANDIDATE_LIMIT
    {
        bail!(
            "Java index candidate limit must be between 1 and {}",
            MAX_JAVA_INDEX_CANDIDATE_LIMIT
        );
    }
    Ok(())
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::Value;

    use super::{analyze_java_index, JavaIndexOptions, JavaResolutionStatus};
    use crate::java_syntax::JavaReferenceKind;

    #[test]
    fn indexes_cross_file_declarations_and_keeps_uncertainty_explicit() {
        let report = analyze_java_index(&corpus_root(), JavaIndexOptions::default())
            .expect("multi-file corpus should index");

        assert!(report.analysis_complete);
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.source.syntax_schema_version, 2);
        assert_eq!(report.source.parsed_files, 10);
        assert!(report
            .symbols
            .iter()
            .any(|symbol| symbol.id == "dev.opticcode.app.Plugin.Inner#run()"));
        assert!(report
            .symbols
            .iter()
            .any(|symbol| symbol.id == "dev.opticcode.app.Plugin.Inner#run(String)"));
        assert!(report.symbols.iter().any(|symbol| {
            symbol.id == "dev.opticcode.model.Material#GUNPOWDER" && symbol.is_static
        }));

        let material = find_reference(&report, JavaReferenceKind::FieldAccess, "GUNPOWDER");
        assert_eq!(material.resolution.status, JavaResolutionStatus::Exact);
        assert_eq!(
            material.resolution.target_id.as_deref(),
            Some("dev.opticcode.model.Material#GUNPOWDER")
        );
        assert_eq!(
            report
                .references
                .iter()
                .filter(|reference| {
                    reference.name == "GUNPOWDER"
                        && normalized_path(&reference.file).ends_with("/Plugin.java")
                })
                .count(),
            1
        );

        let create = find_reference(&report, JavaReferenceKind::MethodInvocation, "create");
        assert_eq!(create.argument_count, Some(1));
        assert_eq!(create.resolution.status, JavaResolutionStatus::Exact);
        assert_eq!(
            create.resolution.target_id.as_deref(),
            Some("dev.opticcode.util.Helpers#create(String)")
        );
        let ping = find_reference(&report, JavaReferenceKind::MethodInvocation, "ping");
        assert_eq!(
            ping.resolution.status,
            JavaResolutionStatus::UniqueCandidate
        );

        let ambiguous = report
            .references
            .iter()
            .find(|reference| {
                reference.kind == JavaReferenceKind::TypeUsage
                    && reference.name == "Duplicate"
                    && normalized_path(&reference.file).ends_with("/Ambiguous.java")
            })
            .expect("ambiguous simple type should be indexed");
        assert_eq!(ambiguous.resolution.status, JavaResolutionStatus::Ambiguous);
        assert_eq!(ambiguous.resolution.candidates.len(), 2);
        assert!(ambiguous.resolution.target_id.is_none());

        let missing = find_reference(&report, JavaReferenceKind::TypeUsage, "MissingType");
        assert_eq!(missing.resolution.status, JavaResolutionStatus::Unresolved);
        let string = find_reference(&report, JavaReferenceKind::TypeUsage, "String");
        assert_eq!(string.resolution.status, JavaResolutionStatus::Exact);
        assert_eq!(
            string.resolution.target_id.as_deref(),
            Some("java.lang.String")
        );
        assert!(report.references.iter().any(|reference| {
            reference.kind == JavaReferenceKind::TypeUsage
                && reference.name == "dev.opticcode.alpha.Duplicate"
                && reference.resolution.status == JavaResolutionStatus::Exact
                && reference.resolution.target_id.as_deref()
                    == Some("dev.opticcode.alpha.Duplicate")
        }));
        assert!(report.references.iter().any(|reference| {
            reference.kind == JavaReferenceKind::FieldAccess
                && normalized_path(&reference.file).ends_with("/ExternalBukkit.java")
                && reference.name == "GUNPOWDER"
                && reference.resolution.status == JavaResolutionStatus::Exact
                && reference.resolution.target_id.as_deref()
                    == Some("org.bukkit.Material#GUNPOWDER")
                && reference
                    .resolution
                    .candidates
                    .first()
                    .is_some_and(|candidate| candidate.external)
        }));
        assert!(report.references.iter().any(|reference| {
            reference.kind == JavaReferenceKind::MethodInvocation
                && reference.name == "broadcastMessage"
                && reference.resolution.status == JavaResolutionStatus::UniqueCandidate
                && reference.resolution.target_id.as_deref()
                    == Some("org.bukkit.Bukkit#broadcastMessage")
        }));
    }

    #[test]
    fn index_output_is_deterministic_and_all_truncations_are_explicit() {
        let first = analyze_java_index(&corpus_root(), JavaIndexOptions::default())
            .expect("first index should succeed");
        let second = analyze_java_index(&corpus_root(), JavaIndexOptions::default())
            .expect("second index should succeed");
        let mut first = serde_json::to_value(first).expect("first report should serialize");
        let mut second = serde_json::to_value(second).expect("second report should serialize");
        remove_timing_fields(&mut first);
        remove_timing_fields(&mut second);
        assert_eq!(first, second);

        let candidate_limited = analyze_java_index(
            &corpus_root(),
            JavaIndexOptions {
                max_candidates_per_reference: 1,
                ..JavaIndexOptions::default()
            },
        )
        .expect("candidate-limited index should succeed");
        let ambiguous = candidate_limited
            .references
            .iter()
            .find(|reference| {
                reference.name == "Duplicate"
                    && reference.resolution.status == JavaResolutionStatus::Ambiguous
            })
            .expect("ambiguous type should remain ambiguous");
        assert_eq!(ambiguous.resolution.candidates.len(), 1);
        assert!(ambiguous.resolution.candidates_truncated);
        assert!(candidate_limited.truncation.candidates);
        assert!(candidate_limited.analysis_complete);

        let symbol_limited = analyze_java_index(
            &corpus_root(),
            JavaIndexOptions {
                max_symbols: 3,
                ..JavaIndexOptions::default()
            },
        )
        .expect("symbol-limited index should succeed");
        assert_eq!(symbol_limited.symbols.len(), 3);
        assert!(symbol_limited.truncation.symbols);
        assert!(symbol_limited.truncated);
    }

    #[test]
    fn candidate_bounds_and_static_arity_rules_are_fail_closed() {
        let root = unique_temp_dir("opticcode java index wildcard bound");
        let mut consumer = String::from("package consumer;\n");
        let owner_root = root.join("owner");
        fs::create_dir_all(&owner_root).expect("owner directory should be created");
        fs::write(
            owner_root.join("Owner.java"),
            concat!(
                "package owner; public class Owner {",
                " public Owner(String value) {}",
                " public void instanceOnly() {}",
                " public static void zero() {}",
                " }\n"
            ),
        )
        .expect("owner source should be written");
        fs::write(
            owner_root.join("DefaultOnly.java"),
            "package owner; public class DefaultOnly {}\n",
        )
        .expect("default constructor source should be written");
        let rival_root = root.join("rival");
        fs::create_dir_all(&rival_root).expect("rival directory should be created");
        fs::write(
            rival_root.join("Rival.java"),
            "package rival; public class Rival { public static int MISSING; }\n",
        )
        .expect("rival source should be written");
        fs::write(root.join("Top.java"), "class Inner {}\n")
            .expect("default-package type should be written");
        fs::write(
            root.join("Outer.java"),
            "class Outer { class Inner {} Inner value; }\n",
        )
        .expect("nested collision source should be written");
        consumer.push_str("import external.External;\n");
        consumer.push_str("import owner.DefaultOnly;\n");
        consumer.push_str("import owner.Owner;\n");
        consumer.push_str("import static owner.Owner.instanceOnly;\n");
        consumer.push_str("import static owner.Owner.zero;\n");
        for index in 0..32 {
            let package_root = root.join(format!("p{index}"));
            fs::create_dir_all(&package_root).expect("package directory should be created");
            fs::write(
                package_root.join("Target.java"),
                format!("package p{index}; public class Target {{}} class String {{}}\n"),
            )
            .expect("candidate source should be written");
            consumer.push_str(&format!("import p{index}.*;\n"));
        }
        consumer.push_str(concat!(
            "final class Consumer { Target target; String text;",
            " void run() { instanceOnly(); zero(1); Owner.zero(1);",
            " new Owner(); new DefaultOnly(1); new External();",
            " int value = Owner.MISSING; } }\n"
        ));
        fs::write(root.join("Consumer.java"), consumer).expect("consumer source should be written");

        let report = analyze_java_index(
            &root,
            JavaIndexOptions {
                max_candidates_per_reference: 2,
                ..JavaIndexOptions::default()
            },
        )
        .expect("wildcard fixture should index");
        let target = find_reference(&report, JavaReferenceKind::TypeUsage, "Target");

        assert_eq!(target.resolution.status, JavaResolutionStatus::Ambiguous);
        assert_eq!(target.resolution.candidates.len(), 2);
        assert!(target.resolution.candidates_truncated);
        let string = find_reference(&report, JavaReferenceKind::TypeUsage, "String");
        assert_eq!(string.resolution.status, JavaResolutionStatus::Ambiguous);
        assert_eq!(string.resolution.candidates.len(), 2);
        assert!(string.resolution.candidates_truncated);
        let instance_only =
            find_reference(&report, JavaReferenceKind::MethodInvocation, "instanceOnly");
        assert_eq!(
            instance_only.resolution.status,
            JavaResolutionStatus::Unresolved
        );
        let zero_calls = report
            .references
            .iter()
            .filter(|reference| {
                reference.kind == JavaReferenceKind::MethodInvocation && reference.name == "zero"
            })
            .collect::<Vec<_>>();
        assert_eq!(zero_calls.len(), 2);
        assert!(zero_calls
            .iter()
            .all(|reference| { reference.resolution.status == JavaResolutionStatus::Unresolved }));
        let constructor = find_reference(&report, JavaReferenceKind::ConstructorCall, "Owner");
        assert_eq!(
            constructor.resolution.status,
            JavaResolutionStatus::Unresolved
        );
        let implicit_mismatch =
            find_reference(&report, JavaReferenceKind::ConstructorCall, "DefaultOnly");
        assert_eq!(
            implicit_mismatch.resolution.status,
            JavaResolutionStatus::Unresolved
        );
        let external_constructor =
            find_reference(&report, JavaReferenceKind::ConstructorCall, "External");
        assert_eq!(
            external_constructor.resolution.status,
            JavaResolutionStatus::UniqueCandidate
        );
        let missing = find_reference(&report, JavaReferenceKind::FieldAccess, "MISSING");
        assert_eq!(missing.resolution.status, JavaResolutionStatus::Unresolved);
        let nested = report
            .references
            .iter()
            .find(|reference| {
                reference.kind == JavaReferenceKind::TypeUsage
                    && reference.name == "Inner"
                    && normalized_path(&reference.file).ends_with("Outer.java")
            })
            .expect("nested type usage should be indexed");
        assert_eq!(nested.resolution.status, JavaResolutionStatus::Exact);
        assert_eq!(nested.resolution.target_id.as_deref(), Some("Outer.Inner"));
        assert!(report.analysis_complete);
        fs::remove_dir_all(root).expect("temporary wildcard fixture should be removed");
    }

    #[test]
    fn crlf_unicode_space_paths_and_invalid_files_are_fail_closed() {
        let root = unique_temp_dir("opticcode java index unicode");
        let source_root = root.join("src main/java/dev/test");
        fs::create_dir_all(&source_root).expect("fixture source root should be created");
        fs::write(
            source_root.join("Peer.java"),
            b"package dev.test; class Peer {}\r\n",
        )
        .expect("peer source should be written");
        let unicode_source = concat!(
            "package dev.test;\r\n",
            "class UnicodeIndex { Object caf\u{00e9} = null; Peer peer; }\r\n"
        );
        let unicode_path = source_root.join("UnicodeIndex.java");
        fs::write(&unicode_path, unicode_source.as_bytes())
            .expect("Unicode source should be written");
        fs::write(
            source_root.join("Broken.java"),
            "package dev.test; class Broken { MissingType value; void run( { }\r\n",
        )
        .expect("broken source should be written");
        let original = fs::read(&unicode_path).expect("fixture should be readable before index");

        let report = analyze_java_index(&root, JavaIndexOptions::default())
            .expect("space path fixture should index");
        let peer = report
            .references
            .iter()
            .find(|reference| {
                reference.kind == JavaReferenceKind::TypeUsage
                    && reference.name == "Peer"
                    && normalized_path(&reference.file).ends_with("/UnicodeIndex.java")
            })
            .expect("Peer type usage should be indexed");
        let broken_references = report
            .references
            .iter()
            .filter(|reference| normalized_path(&reference.file).ends_with("/Broken.java"))
            .collect::<Vec<_>>();

        assert_eq!(peer.resolution.status, JavaResolutionStatus::Exact);
        assert_eq!(
            &unicode_source.as_bytes()[peer.range.start.byte..peer.range.end.byte],
            b"Peer"
        );
        assert!(!broken_references.is_empty());
        assert!(broken_references.iter().all(|reference| {
            reference.resolution.status == JavaResolutionStatus::InvalidSyntaxContext
        }));
        assert!(!report.analysis_complete);
        assert_eq!(
            fs::read(&unicode_path).expect("fixture should remain readable"),
            original
        );
        fs::remove_dir_all(root).expect("temporary index fixture should be removed");
    }

    fn find_reference<'report>(
        report: &'report super::JavaIndexProjectReport,
        kind: JavaReferenceKind,
        name: &str,
    ) -> &'report super::JavaIndexedReference {
        report
            .references
            .iter()
            .find(|reference| reference.kind == kind && reference.name == name)
            .unwrap_or_else(|| panic!("missing {kind:?} reference {name}"))
    }

    fn corpus_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/java-index-mini")
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{label}-{}-{stamp}", std::process::id()))
    }

    fn normalized_path(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    fn remove_timing_fields(value: &mut Value) {
        match value {
            Value::Array(values) => {
                for value in values {
                    remove_timing_fields(value);
                }
            }
            Value::Object(values) => {
                values.remove("timings");
                for value in values.values_mut() {
                    remove_timing_fields(value);
                }
            }
            _ => {}
        }
    }
}
