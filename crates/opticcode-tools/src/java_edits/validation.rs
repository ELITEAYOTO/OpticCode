use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

use super::schema::{
    JavaEditFileValidation, JavaEditProposal, JavaEditRejection, JavaEditRejectionKind,
};
use crate::java_syntax::analyze_java_source;

pub(super) struct SourceSnapshot {
    pub(super) path: PathBuf,
    pub(super) source: String,
    pub(super) source_hash: String,
    pub(super) declared_value_names: BTreeSet<String>,
}

#[derive(Clone, Debug)]
pub(super) enum SnapshotError {
    Invalid(String),
    Changed(String),
}

pub(super) struct FileValidationResult {
    pub(super) validation: Option<JavaEditFileValidation>,
    pub(super) proposed_source: Option<String>,
    pub(super) rejections: Vec<JavaEditRejection>,
    pub(super) reparse_us: u64,
}

pub(super) struct AstSafetyScanner {
    parser: Parser,
}

impl AstSafetyScanner {
    pub(super) fn new() -> Result<Self> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .context("failed to load the Tree-sitter Java grammar for edit validation")?;
        Ok(Self { parser })
    }

    fn declared_value_names(&mut self, source: &str) -> Result<BTreeSet<String>> {
        let tree = self
            .parser
            .parse(source, None)
            .context("Tree-sitter returned no syntax tree during edit validation")?;
        let mut names = BTreeSet::new();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            collect_declared_names(node, source.as_bytes(), &mut names);
            let mut cursor = node.walk();
            stack.extend(node.children(&mut cursor));
        }
        Ok(names)
    }
}

pub(super) fn read_source_snapshot(
    root: &Path,
    relative: &Path,
    expected_hash: &str,
    max_file_bytes: u64,
    scanner: &mut AstSafetyScanner,
) -> std::result::Result<SourceSnapshot, SnapshotError> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(SnapshotError::Invalid(format!(
            "Java edit source path is not a strict relative path: {}",
            relative.display()
        )));
    }

    inspect_regular_path(root, true).map_err(SnapshotError::Invalid)?;
    let mut candidate = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            unreachable!("relative path components were validated")
        };
        candidate.push(part);
        inspect_regular_path(&candidate, false).map_err(SnapshotError::Invalid)?;
    }

    let canonical = fs::canonicalize(&candidate).map_err(|error| {
        SnapshotError::Invalid(format!(
            "failed to resolve Java edit source {}: {error}",
            relative.display()
        ))
    })?;
    if !canonical.starts_with(root) {
        return Err(SnapshotError::Invalid(format!(
            "Java edit source resolves outside the analysis root: {}",
            relative.display()
        )));
    }

    let metadata = fs::symlink_metadata(&candidate).map_err(|error| {
        SnapshotError::Invalid(format!(
            "failed to inspect Java edit source {}: {error}",
            relative.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(SnapshotError::Invalid(format!(
            "Java edit source is no longer a regular file: {}",
            relative.display()
        )));
    }
    if metadata.len() > max_file_bytes {
        return Err(SnapshotError::Invalid(format!(
            "Java edit source exceeds the {} byte limit: {}",
            max_file_bytes,
            relative.display()
        )));
    }

    let file = fs::File::open(&candidate).map_err(|error| {
        SnapshotError::Invalid(format!(
            "failed to open Java edit source {}: {error}",
            relative.display()
        ))
    })?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_file_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            SnapshotError::Invalid(format!(
                "failed to read Java edit source {}: {error}",
                relative.display()
            ))
        })?;
    if bytes.len() as u64 > max_file_bytes {
        return Err(SnapshotError::Changed(format!(
            "Java edit source grew beyond the {} byte limit after indexing: {}",
            max_file_bytes,
            relative.display()
        )));
    }
    inspect_regular_path(&candidate, false).map_err(SnapshotError::Invalid)?;

    let actual_hash = content_hash(&bytes);
    if actual_hash != expected_hash {
        return Err(SnapshotError::Changed(format!(
            "Java edit source changed after indexing: {} (expected {}, found {})",
            relative.display(),
            expected_hash,
            actual_hash
        )));
    }
    let source = String::from_utf8(bytes).map_err(|_| {
        SnapshotError::Changed(format!(
            "Java edit source is no longer valid UTF-8: {}",
            relative.display()
        ))
    })?;
    let declared_value_names = scanner
        .declared_value_names(&source)
        .map_err(|error| SnapshotError::Invalid(format!("{error:#}")))?;

    Ok(SourceSnapshot {
        path: relative.to_path_buf(),
        source,
        source_hash: actual_hash,
        declared_value_names,
    })
}

pub(super) fn validate_file_edits(
    snapshot: &SourceSnapshot,
    proposals: &[JavaEditProposal],
    syntax_valid_before: bool,
) -> FileValidationResult {
    let mut ordered = proposals.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|proposal| {
        (
            proposal.edit_range.start.byte,
            proposal.edit_range.end.byte,
            proposal.id.as_str(),
        )
    });
    let mut rejections = Vec::new();

    for proposal in &ordered {
        if proposal.source_hash != snapshot.source_hash {
            rejections.push(rejection(
                JavaEditRejectionKind::SourceChanged,
                snapshot,
                Some(proposal),
                format!(
                    "proposal source hash {} does not match current snapshot {}",
                    proposal.source_hash, snapshot.source_hash
                ),
            ));
            continue;
        }
        if let Err(message) = validate_proposal_ranges(&snapshot.source, proposal) {
            rejections.push(rejection(
                JavaEditRejectionKind::InvalidRange,
                snapshot,
                Some(proposal),
                message,
            ));
        }
    }

    for pair in ordered.windows(2) {
        let left = pair[0];
        let right = pair[1];
        if right.edit_range.start.byte < left.edit_range.end.byte {
            rejections.push(rejection(
                JavaEditRejectionKind::OverlappingRange,
                snapshot,
                Some(right),
                format!(
                    "edit ranges overlap between proposals {} and {}",
                    left.id, right.id
                ),
            ));
        }
    }

    if !rejections.is_empty() {
        return FileValidationResult {
            validation: None,
            proposed_source: None,
            rejections,
            reparse_us: 0,
        };
    }

    let mut proposed = snapshot.source.clone();
    for proposal in ordered.iter().rev() {
        proposed.replace_range(
            proposal.edit_range.start.byte..proposal.edit_range.end.byte,
            proposal.replacement,
        );
    }

    let reparse_started = Instant::now();
    let parsed = analyze_java_source(snapshot.path.clone(), &proposed);
    let reparse_us = duration_us(reparse_started.elapsed());
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            return FileValidationResult {
                validation: None,
                proposed_source: None,
                rejections: vec![rejection(
                    JavaEditRejectionKind::PostEditSyntaxInvalid,
                    snapshot,
                    ordered.first().copied(),
                    format!("failed to reparse proposed Java source: {error:#}"),
                )],
                reparse_us,
            };
        }
    };

    let validation = JavaEditFileValidation {
        path: snapshot.path.clone(),
        source_hash: snapshot.source_hash.clone(),
        proposed_hash: content_hash(proposed.as_bytes()),
        source_bytes: snapshot.source.len(),
        proposed_bytes: proposed.len(),
        edit_count: ordered.len(),
        proposal_ids: ordered.iter().map(|proposal| proposal.id.clone()).collect(),
        syntax_valid_before,
        syntax_valid_after: parsed.syntax_valid,
        diagnostics_after: parsed.diagnostics.len(),
    };
    if !parsed.syntax_valid {
        rejections.push(rejection(
            JavaEditRejectionKind::PostEditSyntaxInvalid,
            snapshot,
            ordered.first().copied(),
            format!(
                "proposed Java source reparsed with {} diagnostics",
                parsed.diagnostics.len()
            ),
        ));
    }

    FileValidationResult {
        validation: Some(validation),
        proposed_source: Some(proposed),
        rejections,
        reparse_us,
    }
}

pub(super) fn content_hash(bytes: &[u8]) -> String {
    format!("blake3:{}:{}", bytes.len(), blake3::hash(bytes))
}

fn validate_proposal_ranges(source: &str, proposal: &JavaEditProposal) -> Result<(), String> {
    validate_range(
        source,
        proposal.node_range.start.byte,
        proposal.node_range.end.byte,
    )
    .map_err(|message| format!("invalid AST node range: {message}"))?;
    validate_range(
        source,
        proposal.edit_range.start.byte,
        proposal.edit_range.end.byte,
    )
    .map_err(|message| format!("invalid edit range: {message}"))?;
    if proposal.edit_range.start.byte < proposal.node_range.start.byte
        || proposal.edit_range.end.byte > proposal.node_range.end.byte
    {
        return Err("edit range is not contained in the expected AST node range".to_string());
    }
    let actual_node = &source[proposal.node_range.start.byte..proposal.node_range.end.byte];
    if actual_node != proposal.expected_node_content {
        return Err(format!(
            "AST node content mismatch: expected {:?}, found {:?}",
            proposal.expected_node_content, actual_node
        ));
    }
    let actual = &source[proposal.edit_range.start.byte..proposal.edit_range.end.byte];
    if actual != proposal.expected_content {
        return Err(format!(
            "edit content mismatch: expected {:?}, found {:?}",
            proposal.expected_content, actual
        ));
    }
    Ok(())
}

fn validate_range(source: &str, start: usize, end: usize) -> Result<(), String> {
    if start >= end {
        return Err(format!("range must be non-empty: {start}..{end}"));
    }
    if end > source.len() {
        return Err(format!(
            "range {start}..{end} exceeds source length {}",
            source.len()
        ));
    }
    if !source.is_char_boundary(start) || !source.is_char_boundary(end) {
        return Err(format!("range {start}..{end} is not UTF-8 aligned"));
    }
    Ok(())
}

fn rejection(
    kind: JavaEditRejectionKind,
    snapshot: &SourceSnapshot,
    proposal: Option<&JavaEditProposal>,
    message: String,
) -> JavaEditRejection {
    JavaEditRejection {
        kind,
        file: snapshot.path.clone(),
        reference_id: proposal.map(|proposal| proposal.reference_id.clone()),
        rule_id: proposal.map(|proposal| proposal.rule_id),
        message,
    }
}

fn inspect_regular_path(path: &Path, allow_directory: bool) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if metadata_is_link_or_reparse(&metadata) {
        return Err(format!(
            "Java edit source path contains a symlink or reparse point: {}",
            path.display()
        ));
    }
    if !allow_directory && !metadata.is_dir() && !metadata.is_file() {
        return Err(format!(
            "Java edit source path contains a non-regular entry: {}",
            path.display()
        ));
    }
    Ok(())
}

fn collect_declared_names(node: Node<'_>, source: &[u8], names: &mut BTreeSet<String>) {
    const NAMED_DECLARATIONS: &[&str] = &[
        "variable_declarator",
        "formal_parameter",
        "spread_parameter",
        "catch_formal_parameter",
        "type_parameter",
        "record_component",
        "receiver_parameter",
        "enum_constant",
        "type_pattern",
    ];
    if NAMED_DECLARATIONS.contains(&node.kind()) {
        if let Some(name) = node.child_by_field_name("name") {
            insert_node_text(name, source, names);
        } else if node.kind() == "type_parameter" {
            if let Some(name) = node.named_child(0) {
                insert_node_text(name, source, names);
            }
        }
    }
    if node.kind() == "enhanced_for_statement" {
        if let Some(name) = node.child_by_field_name("name") {
            insert_node_text(name, source, names);
        }
    }
    if node.kind() == "inferred_parameters" {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "identifier" {
                insert_node_text(child, source, names);
            }
        }
    }
    if node.kind() == "type_parameters" {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if matches!(child.kind(), "type_identifier" | "identifier") {
                insert_node_text(child, source, names);
            }
        }
    }
    if node.kind() == "lambda_expression" {
        if let Some(parameters) = node.child_by_field_name("parameters") {
            if parameters.kind() == "identifier" {
                insert_node_text(parameters, source, names);
            }
        }
    }
}

fn insert_node_text(node: Node<'_>, source: &[u8], names: &mut BTreeSet<String>) {
    if let Ok(name) = node.utf8_text(source) {
        names.insert(name.to_string());
    }
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

fn duration_us(duration: std::time::Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        content_hash, read_source_snapshot, validate_file_edits, AstSafetyScanner, SnapshotError,
        SourceSnapshot,
    };
    use crate::java_edits::schema::{JavaEditConfidence, JavaEditProposal, JavaEditRejectionKind};
    use crate::java_syntax::{JavaReferenceKind, SourcePoint, SourceRange};

    #[test]
    fn ast_safety_scanner_detects_value_and_type_parameter_shadows() {
        let source = concat!(
            "class Demo<Material> {\n",
            "  Object EntityType;\n",
            "  void run(Object org) {\n",
            "    Object local = null;\n",
            "    java.util.function.Function<Object, Object> f = value -> value;\n",
            "  }\n",
            "}\n",
        );
        let mut scanner = AstSafetyScanner::new().expect("scanner");
        let names = scanner.declared_value_names(source).expect("names");

        for expected in ["Material", "EntityType", "org", "local", "value"] {
            assert!(names.contains(expected), "missing {expected}: {names:?}");
        }
        assert!(!names.contains("Demo"));
        assert!(!names.contains("run"));
    }

    #[test]
    fn edit_validation_applies_backwards_and_reparses_without_writing() {
        let source = "class Demo { Object a = Type.ONE; Object b = Type.TWO; }\n";
        let snapshot = snapshot(source);
        let first = proposal(source, "Type.ONE", "ONE", "FIRST", "LEGACY_ONE");
        let second = proposal(source, "Type.TWO", "TWO", "SECOND", "LEGACY_TWO");

        let result = validate_file_edits(&snapshot, &[first, second], true);
        assert!(result.rejections.is_empty());
        let validation = result.validation.expect("validated preview");
        assert!(validation.syntax_valid_after);
        assert_eq!(validation.edit_count, 2);
        assert_ne!(validation.source_hash, validation.proposed_hash);
        assert!(result.reparse_us > 0);
    }

    #[test]
    fn edit_validation_rejects_overlaps_expected_mismatch_and_utf8_splits() {
        let source = "class Caf\u{00e9} { Object a = Type.ONE; }\n";
        let snapshot = snapshot(source);
        let first = proposal(source, "Type.ONE", "ONE", "FIRST", "LEGACY_ONE");
        let mut overlap = first.clone();
        overlap.id = "SECOND".to_string();
        overlap.reference_id = "SECOND-REFERENCE".to_string();
        let overlap_result = validate_file_edits(&snapshot, &[first.clone(), overlap], true);
        assert!(overlap_result.validation.is_none());
        assert!(overlap_result
            .rejections
            .iter()
            .any(|rejection| rejection.kind == JavaEditRejectionKind::OverlappingRange));

        let mut mismatch = first.clone();
        mismatch.expected_content = "TWO".to_string();
        let mismatch_result = validate_file_edits(&snapshot, &[mismatch], true);
        assert!(mismatch_result.validation.is_none());
        assert!(mismatch_result
            .rejections
            .iter()
            .any(|rejection| rejection.kind == JavaEditRejectionKind::InvalidRange));

        let mut split = first;
        let unicode = source.find('\u{00e9}').expect("unicode byte");
        split.edit_range.start.byte = unicode + 1;
        split.edit_range.end.byte = unicode + 2;
        let split_result = validate_file_edits(&snapshot, &[split], true);
        assert!(split_result.validation.is_none());
        assert!(split_result
            .rejections
            .iter()
            .any(|rejection| rejection.kind == JavaEditRejectionKind::InvalidRange));
    }

    #[test]
    fn source_snapshot_rejects_hash_drift_and_path_traversal() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "opticcode-java-edit-snapshot-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create snapshot fixture");
        let source = "class Demo { Object value = Type.ONE; }\n";
        fs::write(root.join("Demo.java"), source).expect("write snapshot fixture");
        let root = fs::canonicalize(root).expect("canonical fixture root");
        let mut scanner = AstSafetyScanner::new().expect("scanner");

        let drift = read_source_snapshot(
            &root,
            Path::new("Demo.java"),
            "blake3:0:stale",
            1024,
            &mut scanner,
        );
        assert!(matches!(drift, Err(SnapshotError::Changed(_))));

        let valid = read_source_snapshot(
            &root,
            Path::new("Demo.java"),
            &content_hash(source.as_bytes()),
            1024,
            &mut scanner,
        )
        .expect("matching source snapshot");
        assert_eq!(valid.source, source);

        let traversal = read_source_snapshot(
            &root,
            Path::new("../outside.java"),
            &content_hash(source.as_bytes()),
            1024,
            &mut scanner,
        );
        assert!(matches!(traversal, Err(SnapshotError::Invalid(_))));

        fs::remove_dir_all(&root).expect("remove snapshot fixture");
    }

    fn snapshot(source: &str) -> SourceSnapshot {
        SourceSnapshot {
            path: PathBuf::from("Demo.java"),
            source: source.to_string(),
            source_hash: content_hash(source.as_bytes()),
            declared_value_names: BTreeSet::new(),
        }
    }

    fn proposal(
        source: &str,
        node: &str,
        member: &str,
        id: &str,
        replacement: &'static str,
    ) -> JavaEditProposal {
        let node_start = source.find(node).expect("node range");
        let member_offset = node.find(member).expect("member range");
        let member_start = node_start + member_offset;
        JavaEditProposal {
            id: id.to_string(),
            rule_id: "TEST-RULE",
            file: PathBuf::from("Demo.java"),
            source_hash: content_hash(source.as_bytes()),
            reference_id: format!("{id}-REFERENCE"),
            target_id: "dev.test.Type#VALUE".to_string(),
            reference_kind: JavaReferenceKind::FieldAccess,
            node_range: range(node_start, node_start + node.len()),
            edit_range: range(member_start, member_start + member.len()),
            expected_node_content: node.to_string(),
            expected_content: member.to_string(),
            replacement,
            reason: "test",
            resolution_reason: "test exact target".to_string(),
            confidence: JavaEditConfidence::SyntaxExact,
        }
    }

    fn range(start: usize, end: usize) -> SourceRange {
        SourceRange {
            start: SourcePoint {
                byte: start,
                row: 0,
                column: start,
            },
            end: SourcePoint {
                byte: end,
                row: 0,
                column: end,
            },
        }
    }
}
