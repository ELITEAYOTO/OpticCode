//! Read-only Java syntax analysis backed by Tree-sitter.

mod diagnostics;
mod parser;
mod symbols;

use std::cell::Cell;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use walkdir::{DirEntry, WalkDir};

use parser::JavaSyntaxParser;

pub const JAVA_SYNTAX_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_JAVA_SYNTAX_FILE_LIMIT: usize = 500;
pub const MAX_JAVA_SYNTAX_FILE_LIMIT: usize = 5_000;
pub const DEFAULT_JAVA_SYNTAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_JAVA_SYNTAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_JAVA_SYNTAX_ITEM_LIMIT: usize = 2_000;
pub const MAX_JAVA_SYNTAX_ITEM_LIMIT: usize = 20_000;
pub const MAX_JAVA_SYNTAX_WARNINGS: usize = 100;

#[derive(Debug, Clone, Copy)]
pub struct JavaSyntaxOptions {
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_items_per_kind: usize,
}

impl Default for JavaSyntaxOptions {
    fn default() -> Self {
        Self {
            max_files: DEFAULT_JAVA_SYNTAX_FILE_LIMIT,
            max_file_bytes: DEFAULT_JAVA_SYNTAX_FILE_BYTES,
            max_items_per_kind: DEFAULT_JAVA_SYNTAX_ITEM_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct JavaSyntaxLimits {
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_items_per_kind: usize,
    pub max_warnings: usize,
}

impl From<JavaSyntaxOptions> for JavaSyntaxLimits {
    fn from(options: JavaSyntaxOptions) -> Self {
        Self {
            max_files: options.max_files,
            max_file_bytes: options.max_file_bytes,
            max_items_per_kind: options.max_items_per_kind,
            max_warnings: MAX_JAVA_SYNTAX_WARNINGS,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JavaSyntaxCounts {
    pub imports: usize,
    pub symbols: usize,
    pub references: usize,
    pub excluded_regions: usize,
    pub diagnostics: usize,
}

impl JavaSyntaxCounts {
    fn add_assign(&mut self, other: &Self) {
        self.imports = self.imports.saturating_add(other.imports);
        self.symbols = self.symbols.saturating_add(other.symbols);
        self.references = self.references.saturating_add(other.references);
        self.excluded_regions = self.excluded_regions.saturating_add(other.excluded_regions);
        self.diagnostics = self.diagnostics.saturating_add(other.diagnostics);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaSyntaxProjectReport {
    pub schema_version: u32,
    pub operation: &'static str,
    pub root: PathBuf,
    pub input: PathBuf,
    pub limits: JavaSyntaxLimits,
    pub discovered_files: usize,
    pub selected_files: usize,
    pub parsed_files: usize,
    pub syntax_error_files: usize,
    pub skipped_large_files: usize,
    pub skipped_non_utf8_files: usize,
    pub skipped_linked_entries: usize,
    pub walk_errors: usize,
    pub read_errors: usize,
    pub file_selection_truncated: bool,
    pub retained_items_truncated: bool,
    pub warnings_truncated: bool,
    pub truncated: bool,
    pub analysis_complete: bool,
    pub total_bytes: u64,
    pub duration_us: u64,
    pub counts: JavaSyntaxCounts,
    pub files: Vec<JavaSyntaxFileReport>,
    pub warnings: Vec<String>,
}

impl JavaSyntaxProjectReport {
    pub fn syntax_valid(&self) -> bool {
        self.analysis_complete && self.syntax_error_files == 0
    }

    pub fn to_display_string(&self) -> String {
        let mut output = String::new();
        output.push_str("Java syntax analysis (Tree-sitter):\n");
        output.push_str(&format!("- root: {}\n", self.root.display()));
        output.push_str(&format!("- discovered files: {}\n", self.discovered_files));
        output.push_str(&format!("- parsed files: {}\n", self.parsed_files));
        output.push_str(&format!(
            "- syntax error files: {}\n",
            self.syntax_error_files
        ));
        output.push_str(&format!(
            "- analysis complete: {}\n",
            self.analysis_complete
        ));
        output.push_str(&format!("- symbols: {}\n", self.counts.symbols));
        output.push_str(&format!("- imports: {}\n", self.counts.imports));
        output.push_str(&format!("- references: {}\n", self.counts.references));
        output.push_str(&format!(
            "- excluded comments/strings: {}\n",
            self.counts.excluded_regions
        ));
        output.push_str(&format!("- truncated: {}\n", self.truncated));
        output.push_str(&format!(
            "  - file selection: {}\n",
            self.file_selection_truncated
        ));
        output.push_str(&format!(
            "  - retained items: {}\n",
            self.retained_items_truncated
        ));
        output.push_str(&format!("  - warnings: {}\n", self.warnings_truncated));
        output.push_str(&format!(
            "- duration: {:.3} ms\n",
            self.duration_us as f64 / 1_000.0
        ));

        for file in &self.files {
            output.push_str(&format!(
                "\n{}: symbols={}, references={}, diagnostics={}, valid={}\n",
                file.path.display(),
                file.counts.symbols,
                file.counts.references,
                file.counts.diagnostics,
                file.syntax_valid
            ));
            for symbol in &file.symbols {
                output.push_str(&format!(
                    "  - {} {} @ {}:{}\n",
                    symbol.kind.as_str(),
                    symbol.qualified_name,
                    symbol.name_range.start.row,
                    symbol.name_range.start.column
                ));
            }
        }
        for warning in &self.warnings {
            output.push_str(&format!("Warning: {warning}\n"));
        }
        output
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaSyntaxFileReport {
    pub path: PathBuf,
    pub bytes: u64,
    pub content_hash: String,
    pub root_kind: String,
    pub syntax_valid: bool,
    pub retained_items_truncated: bool,
    pub parse_duration_us: u64,
    pub analysis_duration_us: u64,
    pub package: Option<JavaPackage>,
    pub imports: Vec<JavaImport>,
    pub symbols: Vec<JavaSymbol>,
    pub references: Vec<JavaReference>,
    pub excluded_regions: Vec<JavaExcludedRegion>,
    pub diagnostics: Vec<JavaDiagnostic>,
    pub counts: JavaSyntaxCounts,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaPackage {
    pub name: String,
    pub range: SourceRange,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaImport {
    pub path: String,
    pub is_static: bool,
    pub wildcard: bool,
    pub range: SourceRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaSymbolKind {
    Class,
    Interface,
    Enum,
    AnnotationType,
    Record,
    Method,
    Constructor,
    Field,
    EnumConstant,
}

impl JavaSymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Enum => "enum",
            Self::AnnotationType => "annotation_type",
            Self::Record => "record",
            Self::Method => "method",
            Self::Constructor => "constructor",
            Self::Field => "field",
            Self::EnumConstant => "enum_constant",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaSymbol {
    pub kind: JavaSymbolKind,
    pub name: String,
    pub qualified_name: String,
    pub container: Option<String>,
    pub modifiers: Vec<String>,
    pub annotations: Vec<String>,
    pub value_type: Option<String>,
    pub parameters: Vec<JavaParameter>,
    pub signature: Option<String>,
    pub range: SourceRange,
    pub name_range: SourceRange,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaParameter {
    pub name: String,
    pub value_type: Option<String>,
    pub variadic: bool,
    pub range: SourceRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaReferenceKind {
    MethodInvocation,
    FieldAccess,
    ConstructorCall,
    MethodReference,
    Annotation,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaReference {
    pub kind: JavaReferenceKind,
    pub name: String,
    pub qualifier: Option<String>,
    pub container: Option<String>,
    pub range: SourceRange,
    pub name_range: SourceRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaExcludedRegionKind {
    LineComment,
    BlockComment,
    StringLiteral,
    CharacterLiteral,
    TextBlock,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaExcludedRegion {
    pub kind: JavaExcludedRegionKind,
    pub range: SourceRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaDiagnosticKind {
    SyntaxError,
    MissingNode,
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaDiagnostic {
    pub kind: JavaDiagnosticKind,
    pub message: String,
    pub node_kind: String,
    pub range: SourceRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SourcePoint {
    pub byte: usize,
    pub row: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SourceRange {
    pub start: SourcePoint,
    pub end: SourcePoint,
}

pub fn analyze_java_source(path: impl Into<PathBuf>, source: &str) -> Result<JavaSyntaxFileReport> {
    let mut parser = JavaSyntaxParser::new()?;
    parser.parse(path.into(), source, DEFAULT_JAVA_SYNTAX_ITEM_LIMIT)
}

pub fn analyze_java_syntax(
    input: &Path,
    options: JavaSyntaxOptions,
) -> Result<JavaSyntaxProjectReport> {
    validate_options(options)?;
    let started_at = Instant::now();
    let input_metadata = fs::symlink_metadata(input)
        .with_context(|| format!("failed to inspect Java syntax input: {}", input.display()))?;
    if metadata_is_link_or_reparse(&input_metadata) {
        bail!(
            "Java syntax input must not be a symlink or reparse point: {}",
            input.display()
        );
    }
    let input = fs::canonicalize(input)
        .with_context(|| format!("failed to resolve Java syntax input: {}", input.display()))?;
    let mut collection = collect_java_paths(&input)?;
    let root = collection.root;
    let mut paths = collection.paths;
    paths.sort_by_key(|left| normalized_path(left));
    let discovered_files = paths.len();
    let file_selection_truncated = discovered_files > options.max_files;
    paths.truncate(options.max_files);
    let selected_files = paths.len();
    if file_selection_truncated {
        push_warning(
            &mut collection.warnings,
            &mut collection.warnings_truncated,
            format!(
                "Java file limit reached: selected {} of {} files",
                selected_files, discovered_files
            ),
        );
    }
    if collection.skipped_linked_entries > 0 {
        push_warning(
            &mut collection.warnings,
            &mut collection.warnings_truncated,
            format!(
                "skipped {} symlink, reparse point, or entry with unreadable metadata",
                collection.skipped_linked_entries
            ),
        );
    }

    let mut parser = JavaSyntaxParser::new()?;
    let mut report = JavaSyntaxProjectReport {
        schema_version: JAVA_SYNTAX_SCHEMA_VERSION,
        operation: "java_syntax",
        root: root.clone(),
        input,
        limits: options.into(),
        discovered_files,
        selected_files,
        parsed_files: 0,
        syntax_error_files: 0,
        skipped_large_files: 0,
        skipped_non_utf8_files: 0,
        skipped_linked_entries: collection.skipped_linked_entries,
        walk_errors: collection.walk_errors,
        read_errors: 0,
        file_selection_truncated,
        retained_items_truncated: false,
        warnings_truncated: collection.warnings_truncated,
        truncated: file_selection_truncated || collection.warnings_truncated,
        analysis_complete: false,
        total_bytes: 0,
        duration_us: 0,
        counts: JavaSyntaxCounts::default(),
        files: Vec::with_capacity(selected_files),
        warnings: collection.warnings,
    };

    for path in paths {
        let relative = path.strip_prefix(&root).unwrap_or(&path).to_path_buf();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                report.read_errors += 1;
                push_report_warning(
                    &mut report,
                    format!("failed to inspect {}: {error}", relative.display()),
                );
                continue;
            }
        };
        if metadata_is_link_or_reparse(&metadata) {
            report.skipped_linked_entries += 1;
            push_report_warning(
                &mut report,
                format!(
                    "skipped Java symlink or reparse point {}",
                    relative.display()
                ),
            );
            continue;
        }
        if !metadata.is_file() {
            report.read_errors += 1;
            push_report_warning(
                &mut report,
                format!("Java input stopped being a file: {}", relative.display()),
            );
            continue;
        }
        if metadata.len() > options.max_file_bytes {
            report.skipped_large_files += 1;
            push_report_warning(
                &mut report,
                format!(
                    "skipped oversized Java file {} ({} bytes)",
                    relative.display(),
                    metadata.len()
                ),
            );
            continue;
        }
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) => {
                report.read_errors += 1;
                push_report_warning(
                    &mut report,
                    format!("failed to read {}: {error}", relative.display()),
                );
                continue;
            }
        };
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        let mut bounded_reader = file.take(options.max_file_bytes + 1);
        if let Err(error) = bounded_reader.read_to_end(&mut bytes) {
            report.read_errors += 1;
            push_report_warning(
                &mut report,
                format!("failed to read {}: {error}", relative.display()),
            );
            continue;
        }
        if bytes.len() as u64 > options.max_file_bytes {
            report.skipped_large_files += 1;
            push_report_warning(
                &mut report,
                format!(
                    "skipped Java file {} after it grew beyond {} bytes",
                    relative.display(),
                    options.max_file_bytes
                ),
            );
            continue;
        }
        let source = match std::str::from_utf8(&bytes) {
            Ok(source) => source,
            Err(_) => {
                report.skipped_non_utf8_files += 1;
                push_report_warning(
                    &mut report,
                    format!("skipped non-UTF-8 Java file {}", relative.display()),
                );
                continue;
            }
        };
        match parser.parse(relative.clone(), source, options.max_items_per_kind) {
            Ok(file) => {
                report.parsed_files += 1;
                report.total_bytes = report.total_bytes.saturating_add(file.bytes);
                if !file.syntax_valid {
                    report.syntax_error_files += 1;
                }
                if file.retained_items_truncated {
                    report.retained_items_truncated = true;
                }
                report.counts.add_assign(&file.counts);
                report.files.push(file);
            }
            Err(error) => {
                report.read_errors += 1;
                push_report_warning(
                    &mut report,
                    format!("failed to parse {}: {error:#}", relative.display()),
                );
            }
        }
    }

    report.truncated = report.file_selection_truncated
        || report.retained_items_truncated
        || report.warnings_truncated;
    report.analysis_complete = report.parsed_files == report.discovered_files
        && report.skipped_large_files == 0
        && report.skipped_non_utf8_files == 0
        && report.skipped_linked_entries == 0
        && report.walk_errors == 0
        && report.read_errors == 0
        && !report.truncated;
    report.duration_us = duration_us(started_at.elapsed());
    Ok(report)
}

struct JavaPathCollection {
    root: PathBuf,
    paths: Vec<PathBuf>,
    warnings: Vec<String>,
    warnings_truncated: bool,
    skipped_linked_entries: usize,
    walk_errors: usize,
}

fn collect_java_paths(input: &Path) -> Result<JavaPathCollection> {
    if input.is_file() {
        if input.extension().and_then(|extension| extension.to_str()) != Some("java") {
            bail!("Java syntax input file must use the .java extension");
        }
        let root = input
            .parent()
            .context("Java source file has no parent directory")?
            .to_path_buf();
        return Ok(JavaPathCollection {
            root,
            paths: vec![input.to_path_buf()],
            warnings: Vec::new(),
            warnings_truncated: false,
            skipped_linked_entries: 0,
            walk_errors: 0,
        });
    }
    if !input.is_dir() {
        bail!("Java syntax input is neither a file nor a directory");
    }

    let mut paths = Vec::new();
    let mut warnings = Vec::new();
    let mut warnings_truncated = false;
    let mut walk_errors = 0usize;
    let skipped_linked_entries = Cell::new(0usize);
    for entry in WalkDir::new(input)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| should_enter(entry, &skipped_linked_entries))
    {
        match entry {
            Ok(entry)
                if entry.file_type().is_file()
                    && entry.path().extension().and_then(|value| value.to_str())
                        == Some("java") =>
            {
                paths.push(entry.into_path());
            }
            Ok(_) => {}
            Err(error) => {
                walk_errors = walk_errors.saturating_add(1);
                push_warning(
                    &mut warnings,
                    &mut warnings_truncated,
                    format!("walk error: {error}"),
                );
            }
        }
    }
    Ok(JavaPathCollection {
        root: input.to_path_buf(),
        paths,
        warnings,
        warnings_truncated,
        skipped_linked_entries: skipped_linked_entries.get(),
        walk_errors,
    })
}

fn should_enter(entry: &DirEntry, skipped_linked_entries: &Cell<usize>) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    let unsafe_entry = fs::symlink_metadata(entry.path())
        .map(|metadata| metadata_is_link_or_reparse(&metadata))
        .unwrap_or(true);
    if unsafe_entry {
        skipped_linked_entries.set(skipped_linked_entries.get().saturating_add(1));
        return false;
    }
    if !entry.file_type().is_dir() {
        return true;
    }
    !matches!(
        entry.file_name().to_string_lossy().as_ref(),
        ".git"
            | ".idea"
            | ".gradle"
            | ".opticcode"
            | "target"
            | "build"
            | "out"
            | "bin"
            | "classes"
            | "node_modules"
    )
}

fn validate_options(options: JavaSyntaxOptions) -> Result<()> {
    if options.max_files == 0 || options.max_files > MAX_JAVA_SYNTAX_FILE_LIMIT {
        bail!(
            "Java syntax file limit must be between 1 and {}",
            MAX_JAVA_SYNTAX_FILE_LIMIT
        );
    }
    if options.max_file_bytes == 0 || options.max_file_bytes > MAX_JAVA_SYNTAX_FILE_BYTES {
        bail!(
            "Java syntax file byte limit must be between 1 and {}",
            MAX_JAVA_SYNTAX_FILE_BYTES
        );
    }
    if options.max_items_per_kind == 0 || options.max_items_per_kind > MAX_JAVA_SYNTAX_ITEM_LIMIT {
        bail!(
            "Java syntax item limit must be between 1 and {}",
            MAX_JAVA_SYNTAX_ITEM_LIMIT
        );
    }
    Ok(())
}

fn push_warning(warnings: &mut Vec<String>, truncated: &mut bool, warning: String) {
    if warnings.len() < MAX_JAVA_SYNTAX_WARNINGS {
        warnings.push(warning);
    } else {
        *truncated = true;
    }
}

fn push_report_warning(report: &mut JavaSyntaxProjectReport, warning: String) {
    push_warning(
        &mut report.warnings,
        &mut report.warnings_truncated,
        warning,
    );
}

#[cfg(windows)]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::{
        analyze_java_source, JavaDiagnosticKind, JavaExcludedRegionKind, JavaReferenceKind,
        JavaSymbolKind,
    };

    #[test]
    fn extracts_java_symbols_references_and_positions() {
        let source = r#"
package dev.opticcode.test;

import org.bukkit.Material;
import org.bukkit.event.EventHandler;
import static java.util.Collections.*;

@Deprecated
public final class Demo {
    private Material material = Material.GUNPOWDER;

    public Demo() {}

    @EventHandler
    public void run(String value) {
        getServer().broadcastMessage(value);
        new StringBuilder();
    }

    enum State { READY, DONE }
    interface Handler {}
    @interface Marker {}
}
"#;

        let report = analyze_java_source("Demo.java", source).expect("Java should parse");

        assert!(report.syntax_valid);
        assert_eq!(
            report.package.as_ref().map(|package| package.name.as_str()),
            Some("dev.opticcode.test")
        );
        assert!(report
            .imports
            .iter()
            .any(|import| import.path == "org.bukkit.Material"));
        assert!(report
            .imports
            .iter()
            .any(|import| import.is_static && import.wildcard));
        assert!(report.symbols.iter().any(|symbol| {
            symbol.kind == JavaSymbolKind::Class
                && symbol.qualified_name == "dev.opticcode.test.Demo"
                && symbol.annotations.iter().any(|value| value == "Deprecated")
        }));
        assert!(report
            .symbols
            .iter()
            .any(|symbol| symbol.kind == JavaSymbolKind::Field && symbol.name == "material"));
        assert!(report.symbols.iter().any(|symbol| {
            symbol.kind == JavaSymbolKind::Method
                && symbol.name == "run"
                && symbol.parameters.len() == 1
                && symbol
                    .annotations
                    .iter()
                    .any(|value| value == "EventHandler")
        }));
        assert!(report
            .symbols
            .iter()
            .any(|symbol| symbol.kind == JavaSymbolKind::Interface));
        assert!(report
            .symbols
            .iter()
            .any(|symbol| symbol.kind == JavaSymbolKind::AnnotationType));
        assert!(report.references.iter().any(|reference| {
            reference.kind == JavaReferenceKind::FieldAccess
                && reference.qualifier.as_deref() == Some("Material")
                && reference.name == "GUNPOWDER"
        }));
        assert!(report.references.iter().any(|reference| {
            reference.kind == JavaReferenceKind::MethodInvocation
                && reference.name == "broadcastMessage"
        }));
        assert!(report
            .references
            .iter()
            .any(|reference| reference.kind == JavaReferenceKind::ConstructorCall));
        assert!(report
            .symbols
            .iter()
            .all(|symbol| symbol.range.start.byte <= symbol.name_range.start.byte));
    }

    #[test]
    fn comments_and_strings_are_excluded_from_code_references() {
        let source = r#"
class FalsePositiveFixture {
    // Material.GUNPOWDER must not be edited here.
    String text = "Material.GUNPOWDER";
    char marker = 'x';
    Object material = Material.GUNPOWDER;
}
"#;

        let report =
            analyze_java_source("FalsePositiveFixture.java", source).expect("Java should parse");
        let legacy_references = report
            .references
            .iter()
            .filter(|reference| {
                reference.kind == JavaReferenceKind::FieldAccess
                    && reference.qualifier.as_deref() == Some("Material")
                    && reference.name == "GUNPOWDER"
            })
            .count();

        assert_eq!(legacy_references, 1);
        assert!(report
            .excluded_regions
            .iter()
            .any(|region| region.kind == JavaExcludedRegionKind::LineComment));
        assert!(report
            .excluded_regions
            .iter()
            .any(|region| region.kind == JavaExcludedRegionKind::StringLiteral));
        assert!(report
            .excluded_regions
            .iter()
            .any(|region| region.kind == JavaExcludedRegionKind::CharacterLiteral));
    }

    #[test]
    fn byte_ranges_are_exact_with_crlf_and_unicode() {
        let source = concat!(
            "class UnicodeRanges {\r\n",
            "    Object caf\u{00e9} = null; Object value = Material.GUNPOWDER;\r\n",
            "}\r\n"
        );
        let report = analyze_java_source("UnicodeRanges.java", source).expect("Java should parse");
        let reference = report
            .references
            .iter()
            .find(|reference| {
                reference.kind == JavaReferenceKind::FieldAccess
                    && reference.qualifier.as_deref() == Some("Material")
                    && reference.name == "GUNPOWDER"
            })
            .expect("real Material access should be retained");
        let expected_start = source
            .find("Material.GUNPOWDER")
            .expect("fixture should contain the target");
        let expected_end = expected_start + "Material.GUNPOWDER".len();
        let line_start = source[..expected_start]
            .rfind('\n')
            .map_or(0, |position| position + 1);

        assert_eq!(reference.range.start.byte, expected_start);
        assert_eq!(reference.range.end.byte, expected_end);
        assert_eq!(reference.range.start.row, 1);
        assert_eq!(reference.range.start.column, expected_start - line_start);
        assert_eq!(
            &source.as_bytes()[reference.range.start.byte..reference.range.end.byte],
            b"Material.GUNPOWDER"
        );
    }

    #[test]
    fn nested_overloaded_and_anonymous_declarations_are_retained() {
        let source = r#"
package dev.opticcode.test;

class Outer {
    class Nested {}

    void run() {}
    void run(String value) {}

    Runnable task = new Runnable() {
        public void run() {}
    };
}
"#;
        let report = analyze_java_source("Outer.java", source).expect("Java should parse");
        let run_signatures = report
            .symbols
            .iter()
            .filter(|symbol| symbol.kind == JavaSymbolKind::Method && symbol.name == "run")
            .filter_map(|symbol| symbol.signature.as_deref())
            .collect::<Vec<_>>();

        assert!(report.syntax_valid);
        assert!(report.symbols.iter().any(|symbol| {
            symbol.kind == JavaSymbolKind::Class
                && symbol.qualified_name == "dev.opticcode.test.Outer.Nested"
        }));
        assert!(run_signatures.contains(&"run()"));
        assert!(run_signatures.contains(&"run(String)"));
        assert!(report.references.iter().any(|reference| {
            reference.kind == JavaReferenceKind::ConstructorCall && reference.name == "Runnable"
        }));
    }

    #[test]
    fn text_blocks_are_excluded_from_code_references() {
        let source = concat!(
            "class TextBlockFixture {\n",
            "    String text = \"\"\"\n",
            "        Material.GUNPOWDER\n",
            "        \"\"\";\n",
            "    Object value = Material.GUNPOWDER;\n",
            "}\n"
        );
        let report =
            analyze_java_source("TextBlockFixture.java", source).expect("Java should parse");
        let legacy_references = report
            .references
            .iter()
            .filter(|reference| {
                reference.kind == JavaReferenceKind::FieldAccess
                    && reference.qualifier.as_deref() == Some("Material")
                    && reference.name == "GUNPOWDER"
            })
            .count();

        assert!(report.syntax_valid);
        assert_eq!(legacy_references, 1);
        assert!(report
            .excluded_regions
            .iter()
            .any(|region| region.kind == JavaExcludedRegionKind::TextBlock));
    }

    #[test]
    fn malformed_java_returns_structured_diagnostics() {
        let report = analyze_java_source("Broken.java", "class Broken { void run( { return }")
            .expect("Tree-sitter should return a recoverable tree");
        let error_report = analyze_java_source(
            "Error.java",
            "class Error { void run() { int value = ???; } }",
        )
        .expect("Tree-sitter should retain an ERROR node");

        assert!(!report.syntax_valid);
        assert!(!report.diagnostics.is_empty());
        assert!(error_report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == JavaDiagnosticKind::SyntaxError));
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == JavaDiagnosticKind::MissingNode));
        assert!(report
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.range.start.byte <= diagnostic.range.end.byte));
        assert!(error_report.references.iter().all(|reference| {
            error_report.diagnostics.iter().all(|diagnostic| {
                diagnostic.kind != JavaDiagnosticKind::SyntaxError
                    || reference.range.start.byte < diagnostic.range.start.byte
                    || reference.range.end.byte > diagnostic.range.end.byte
            })
        }));
    }

    #[test]
    fn warning_retention_reports_its_own_truncation() {
        use super::{push_warning, MAX_JAVA_SYNTAX_WARNINGS};

        let mut warnings = Vec::new();
        let mut truncated = false;
        for index in 0..=MAX_JAVA_SYNTAX_WARNINGS {
            push_warning(&mut warnings, &mut truncated, format!("warning {index}"));
        }

        assert_eq!(warnings.len(), MAX_JAVA_SYNTAX_WARNINGS);
        assert!(truncated);
    }

    #[cfg(windows)]
    #[test]
    fn directory_scan_skips_junctions_and_rejects_a_junction_root() {
        use std::fs;
        use std::process::Command;
        use std::time::{SystemTime, UNIX_EPOCH};

        use super::{analyze_java_syntax, JavaSyntaxOptions};

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let fixture = std::env::temp_dir().join(format!(
            "opticcode-java-syntax-junction-{}-{stamp}",
            std::process::id()
        ));
        let root = fixture.join("root");
        let external = fixture.join("external");
        let junction = root.join("linked");
        fs::create_dir_all(&root).expect("scan root should be created");
        fs::create_dir_all(&external).expect("external root should be created");
        fs::write(external.join("Outside.java"), "class Outside {}\n")
            .expect("external Java fixture should be written");
        let output = Command::new("cmd")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&external)
            .output()
            .expect("junction command should start");
        if !output.status.success() {
            let _ = fs::remove_dir_all(&fixture);
            panic!(
                "junction should be created: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let report = analyze_java_syntax(&root, JavaSyntaxOptions::default())
            .expect("normal root should remain analyzable");
        let root_error = analyze_java_syntax(&junction, JavaSyntaxOptions::default())
            .expect_err("junction root must be rejected");

        fs::remove_dir(&junction).expect("junction should be removed without touching its target");
        fs::remove_dir_all(&fixture).expect("junction fixture should be removed");

        assert_eq!(report.discovered_files, 0);
        assert_eq!(report.skipped_linked_entries, 1);
        assert!(!report.analysis_complete);
        assert!(!report.syntax_valid());
        assert!(format!("{root_error:#}").contains("symlink or reparse point"));
    }
}
