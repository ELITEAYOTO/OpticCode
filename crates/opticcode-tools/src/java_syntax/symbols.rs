use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use tree_sitter::{Node, Tree};

use super::diagnostics::diagnostic_for_node;
use super::{
    JavaDiagnostic, JavaExcludedRegion, JavaExcludedRegionKind, JavaImport, JavaPackage,
    JavaParameter, JavaReference, JavaReferenceKind, JavaSymbol, JavaSymbolKind, JavaSyntaxCounts,
    JavaSyntaxFileReport, SourcePoint, SourceRange,
};

const MAX_TEXT_FIELD_CHARS: usize = 512;

pub(super) fn collect_file_report(
    path: PathBuf,
    source: &str,
    tree: &Tree,
    parse_duration: Duration,
    item_limit: usize,
) -> Result<JavaSyntaxFileReport> {
    let analysis_started = Instant::now();
    let root = tree.root_node();
    let package = extract_package(root, source.as_bytes());
    let (imports, import_count, imports_truncated) =
        extract_imports(root, source.as_bytes(), item_limit);
    let mut collector = Collector {
        source: source.as_bytes(),
        package_name: package.as_ref().map(|package| package.name.clone()),
        item_limit,
        symbols: Vec::new(),
        references: Vec::new(),
        excluded_regions: Vec::new(),
        diagnostics: Vec::new(),
        counts: JavaSyntaxCounts {
            imports: import_count,
            ..JavaSyntaxCounts::default()
        },
        retained_items_truncated: imports_truncated,
    };
    collector.visit(root, &mut Vec::new(), false);

    Ok(JavaSyntaxFileReport {
        path,
        bytes: source.len() as u64,
        content_hash: format!(
            "blake3:{}:{}",
            source.len(),
            blake3::hash(source.as_bytes())
        ),
        root_kind: root.kind().to_string(),
        syntax_valid: !root.has_error() && collector.counts.diagnostics == 0,
        retained_items_truncated: collector.retained_items_truncated,
        parse_duration_us: duration_us(parse_duration),
        analysis_duration_us: duration_us(analysis_started.elapsed()),
        package,
        imports,
        symbols: collector.symbols,
        references: collector.references,
        excluded_regions: collector.excluded_regions,
        diagnostics: collector.diagnostics,
        counts: collector.counts,
    })
}

struct Collector<'source> {
    source: &'source [u8],
    package_name: Option<String>,
    item_limit: usize,
    symbols: Vec<JavaSymbol>,
    references: Vec<JavaReference>,
    excluded_regions: Vec<JavaExcludedRegion>,
    diagnostics: Vec<JavaDiagnostic>,
    counts: JavaSyntaxCounts,
    retained_items_truncated: bool,
}

impl Collector<'_> {
    fn visit(&mut self, node: Node<'_>, containers: &mut Vec<String>, inside_error: bool) {
        if let Some(diagnostic) = diagnostic_for_node(node) {
            self.push_diagnostic(diagnostic);
        }

        if let Some(kind) = excluded_region_kind(node, self.source) {
            self.push_excluded_region(JavaExcludedRegion {
                kind,
                range: source_range(node),
            });
            return;
        }
        let inside_error = inside_error || node.is_error() || node.is_missing();
        if inside_error {
            self.visit_children(node, containers, true);
            return;
        }
        if matches!(node.kind(), "package_declaration" | "import_declaration") {
            return;
        }

        if let Some(kind) = type_symbol_kind(node.kind()) {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = node_text(name_node, self.source);
                self.push_symbol(self.declaration_symbol(kind, node, name_node, &name, containers));
                containers.push(name);
                self.visit_children(node, containers, false);
                containers.pop();
                return;
            }
        }

        match node.kind() {
            "method_declaration" => self.collect_callable(node, containers, false),
            "constructor_declaration" => self.collect_callable(node, containers, true),
            "field_declaration" => self.collect_fields(node, containers),
            "enum_constant" => self.collect_enum_constant(node, containers),
            "method_invocation" => self.collect_method_invocation(node, containers),
            "field_access" => self.collect_field_access(node, containers),
            "object_creation_expression" => self.collect_constructor_call(node, containers),
            "method_reference" => self.collect_method_reference(node, containers),
            "annotation" | "marker_annotation" => self.collect_annotation(node, containers),
            "type_identifier" | "scoped_type_identifier" if is_type_usage_root(node) => {
                self.collect_type_usage(node, containers)
            }
            _ => {}
        }

        self.visit_children(node, containers, false);
    }

    fn visit_children(&mut self, node: Node<'_>, containers: &mut Vec<String>, inside_error: bool) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit(child, containers, inside_error);
        }
    }

    fn declaration_symbol(
        &self,
        kind: JavaSymbolKind,
        node: Node<'_>,
        name_node: Node<'_>,
        name: &str,
        containers: &[String],
    ) -> JavaSymbol {
        JavaSymbol {
            kind,
            name: name.to_string(),
            qualified_name: qualified_name(self.package_name.as_deref(), containers, name),
            container: container_name(containers),
            modifiers: extract_modifiers(node, self.source),
            annotations: extract_annotations(node, self.source),
            value_type: None,
            parameters: Vec::new(),
            signature: Some(name.to_string()),
            range: source_range(node),
            name_range: source_range(name_node),
        }
    }

    fn collect_callable(&mut self, node: Node<'_>, containers: &[String], constructor: bool) {
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        let name = node_text(name_node, self.source);
        let (parameters, parameters_truncated) = node
            .child_by_field_name("parameters")
            .map(|parameters| extract_parameters(parameters, self.source, self.item_limit))
            .unwrap_or_default();
        if parameters_truncated {
            self.retained_items_truncated = true;
        }
        let value_type = if constructor {
            None
        } else {
            node.child_by_field_name("type")
                .map(|value_type| bounded_node_text(value_type, self.source))
        };
        let parameter_types = parameters
            .iter()
            .map(|parameter| parameter.value_type.as_deref().unwrap_or("?"))
            .collect::<Vec<_>>()
            .join(", ");
        self.push_symbol(JavaSymbol {
            kind: if constructor {
                JavaSymbolKind::Constructor
            } else {
                JavaSymbolKind::Method
            },
            name: name.clone(),
            qualified_name: qualified_name(self.package_name.as_deref(), containers, &name),
            container: container_name(containers),
            modifiers: extract_modifiers(node, self.source),
            annotations: extract_annotations(node, self.source),
            value_type,
            parameters,
            signature: Some(format!("{name}({parameter_types})")),
            range: source_range(node),
            name_range: source_range(name_node),
        });
    }

    fn collect_fields(&mut self, node: Node<'_>, containers: &[String]) {
        let value_type = node
            .child_by_field_name("type")
            .map(|value_type| bounded_node_text(value_type, self.source));
        let modifiers = extract_modifiers(node, self.source);
        let annotations = extract_annotations(node, self.source);
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() != "variable_declarator" {
                continue;
            }
            let Some(name_node) = child.child_by_field_name("name") else {
                continue;
            };
            let name = node_text(name_node, self.source);
            self.push_symbol(JavaSymbol {
                kind: JavaSymbolKind::Field,
                name: name.clone(),
                qualified_name: qualified_name(self.package_name.as_deref(), containers, &name),
                container: container_name(containers),
                modifiers: modifiers.clone(),
                annotations: annotations.clone(),
                value_type: value_type.clone(),
                parameters: Vec::new(),
                signature: value_type
                    .as_ref()
                    .map(|value_type| format!("{value_type} {name}")),
                range: source_range(child),
                name_range: source_range(name_node),
            });
        }
    }

    fn collect_enum_constant(&mut self, node: Node<'_>, containers: &[String]) {
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        let name = node_text(name_node, self.source);
        self.push_symbol(JavaSymbol {
            kind: JavaSymbolKind::EnumConstant,
            name: name.clone(),
            qualified_name: qualified_name(self.package_name.as_deref(), containers, &name),
            container: container_name(containers),
            modifiers: extract_modifiers(node, self.source),
            annotations: extract_annotations(node, self.source),
            value_type: None,
            parameters: Vec::new(),
            signature: Some(name),
            range: source_range(node),
            name_range: source_range(name_node),
        });
    }

    fn collect_method_invocation(&mut self, node: Node<'_>, containers: &[String]) {
        self.collect_reference(
            JavaReferenceKind::MethodInvocation,
            node,
            node.child_by_field_name("name"),
            node.child_by_field_name("object"),
            containers,
        );
    }

    fn collect_field_access(&mut self, node: Node<'_>, containers: &[String]) {
        self.collect_reference(
            JavaReferenceKind::FieldAccess,
            node,
            node.child_by_field_name("field"),
            node.child_by_field_name("object"),
            containers,
        );
    }

    fn collect_constructor_call(&mut self, node: Node<'_>, containers: &[String]) {
        self.collect_reference(
            JavaReferenceKind::ConstructorCall,
            node,
            node.child_by_field_name("type"),
            None,
            containers,
        );
    }

    fn collect_method_reference(&mut self, node: Node<'_>, containers: &[String]) {
        let mut cursor = node.walk();
        let named = node.named_children(&mut cursor).collect::<Vec<_>>();
        let name = named.last().copied();
        let qualifier = named.first().copied().filter(|first| Some(*first) != name);
        self.collect_reference(
            JavaReferenceKind::MethodReference,
            node,
            name,
            qualifier,
            containers,
        );
    }

    fn collect_annotation(&mut self, node: Node<'_>, containers: &[String]) {
        self.collect_reference(
            JavaReferenceKind::Annotation,
            node,
            node.child_by_field_name("name"),
            None,
            containers,
        );
    }

    fn collect_type_usage(&mut self, node: Node<'_>, containers: &[String]) {
        let name_node = node.child_by_field_name("name").or(Some(node));
        self.collect_reference(
            JavaReferenceKind::TypeUsage,
            node,
            name_node,
            node.child_by_field_name("scope"),
            containers,
        );
    }

    fn collect_reference(
        &mut self,
        kind: JavaReferenceKind,
        node: Node<'_>,
        name_node: Option<Node<'_>>,
        qualifier_node: Option<Node<'_>>,
        containers: &[String],
    ) {
        let Some(name_node) = name_node else {
            return;
        };
        self.push_reference(JavaReference {
            kind,
            name: bounded_node_text(name_node, self.source),
            qualifier: qualifier_node.map(|qualifier| bounded_node_text(qualifier, self.source)),
            container: container_name(containers),
            argument_count: match kind {
                JavaReferenceKind::MethodInvocation | JavaReferenceKind::ConstructorCall => {
                    node.child_by_field_name("arguments").map(named_child_count)
                }
                _ => None,
            },
            range: source_range(node),
            name_range: source_range(name_node),
        });
    }

    fn push_symbol(&mut self, symbol: JavaSymbol) {
        self.counts.symbols += 1;
        if self.symbols.len() < self.item_limit {
            self.symbols.push(symbol);
        } else {
            self.retained_items_truncated = true;
        }
    }

    fn push_reference(&mut self, reference: JavaReference) {
        self.counts.references += 1;
        if self.references.len() < self.item_limit {
            self.references.push(reference);
        } else {
            self.retained_items_truncated = true;
        }
    }

    fn push_excluded_region(&mut self, region: JavaExcludedRegion) {
        self.counts.excluded_regions += 1;
        if self.excluded_regions.len() < self.item_limit {
            self.excluded_regions.push(region);
        } else {
            self.retained_items_truncated = true;
        }
    }

    fn push_diagnostic(&mut self, diagnostic: JavaDiagnostic) {
        self.counts.diagnostics += 1;
        if self.diagnostics.len() < self.item_limit {
            self.diagnostics.push(diagnostic);
        } else {
            self.retained_items_truncated = true;
        }
    }
}

fn extract_package(root: Node<'_>, source: &[u8]) -> Option<JavaPackage> {
    let mut cursor = root.walk();
    let package = root
        .named_children(&mut cursor)
        .find(|child| child.kind() == "package_declaration")
        .map(|node| {
            let raw = node_text(node, source);
            JavaPackage {
                name: raw
                    .trim_start_matches("package")
                    .trim()
                    .trim_end_matches(';')
                    .trim()
                    .to_string(),
                range: source_range(node),
            }
        });
    package
}

fn extract_imports(
    root: Node<'_>,
    source: &[u8],
    item_limit: usize,
) -> (Vec<JavaImport>, usize, bool) {
    let mut cursor = root.walk();
    let mut count = 0usize;
    let mut imports = Vec::new();
    for node in root
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "import_declaration")
    {
        count += 1;
        if imports.len() >= item_limit {
            continue;
        }
        let raw = node_text(node, source);
        let mut path = raw.trim_start_matches("import").trim();
        let is_static = path.starts_with("static ");
        if is_static {
            path = path.trim_start_matches("static ").trim();
        }
        path = path.trim_end_matches(';').trim();
        let wildcard = path.ends_with(".*");
        imports.push(JavaImport {
            path: path.to_string(),
            is_static,
            wildcard,
            range: source_range(node),
        });
    }
    (imports, count, count > item_limit)
}

fn extract_parameters(
    parameters: Node<'_>,
    source: &[u8],
    item_limit: usize,
) -> (Vec<JavaParameter>, bool) {
    let mut cursor = parameters.walk();
    let mut count = 0usize;
    let mut retained = Vec::new();
    for parameter in parameters.named_children(&mut cursor).filter(|parameter| {
        matches!(
            parameter.kind(),
            "formal_parameter" | "spread_parameter" | "receiver_parameter"
        )
    }) {
        count += 1;
        if retained.len() >= item_limit {
            continue;
        }
        if let Some(name_node) = parameter.child_by_field_name("name") {
            retained.push(JavaParameter {
                name: node_text(name_node, source),
                value_type: parameter
                    .child_by_field_name("type")
                    .map(|value_type| bounded_node_text(value_type, source)),
                variadic: parameter.kind() == "spread_parameter",
                range: source_range(parameter),
            });
        }
    }
    (retained, count > item_limit)
}

fn extract_modifiers(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let Some(modifiers) = find_named_child(node, "modifiers") else {
        return Vec::new();
    };
    let mut values = Vec::new();
    let mut cursor = modifiers.walk();
    for child in modifiers.children(&mut cursor) {
        if matches!(child.kind(), "annotation" | "marker_annotation") {
            continue;
        }
        let value = node_text(child, source).trim().to_string();
        if !value.is_empty() {
            values.push(value);
        }
    }
    values.sort();
    values.dedup();
    values
}

fn extract_annotations(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut annotations = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "annotation" | "marker_annotation" => {
                if let Some(name) = child.child_by_field_name("name") {
                    annotations.push(node_text(name, source));
                }
            }
            "modifiers" => {
                let mut modifiers_cursor = child.walk();
                for modifier in child.named_children(&mut modifiers_cursor) {
                    if matches!(modifier.kind(), "annotation" | "marker_annotation") {
                        if let Some(name) = modifier.child_by_field_name("name") {
                            annotations.push(node_text(name, source));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    annotations.sort();
    annotations.dedup();
    annotations
}

fn find_named_child<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let child = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == kind);
    child
}

fn type_symbol_kind(kind: &str) -> Option<JavaSymbolKind> {
    match kind {
        "class_declaration" => Some(JavaSymbolKind::Class),
        "interface_declaration" => Some(JavaSymbolKind::Interface),
        "enum_declaration" => Some(JavaSymbolKind::Enum),
        "annotation_type_declaration" => Some(JavaSymbolKind::AnnotationType),
        "record_declaration" => Some(JavaSymbolKind::Record),
        _ => None,
    }
}

fn is_type_usage_root(node: Node<'_>) -> bool {
    node.parent()
        .is_none_or(|parent| parent.kind() != "scoped_type_identifier")
}

fn named_child_count(node: Node<'_>) -> usize {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).count()
}

fn excluded_region_kind(node: Node<'_>, source: &[u8]) -> Option<JavaExcludedRegionKind> {
    match node.kind() {
        "line_comment" => Some(JavaExcludedRegionKind::LineComment),
        "block_comment" => Some(JavaExcludedRegionKind::BlockComment),
        "string_literal"
            if source
                .get(node.byte_range())
                .is_some_and(|text| text.starts_with(b"\"\"\"")) =>
        {
            Some(JavaExcludedRegionKind::TextBlock)
        }
        "string_literal" => Some(JavaExcludedRegionKind::StringLiteral),
        "character_literal" => Some(JavaExcludedRegionKind::CharacterLiteral),
        "text_block" => Some(JavaExcludedRegionKind::TextBlock),
        _ => None,
    }
}

fn qualified_name(package: Option<&str>, containers: &[String], name: &str) -> String {
    package
        .into_iter()
        .chain(containers.iter().map(String::as_str))
        .chain(std::iter::once(name))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(".")
}

fn container_name(containers: &[String]) -> Option<String> {
    (!containers.is_empty()).then(|| containers.join("."))
}

fn node_text(node: Node<'_>, source: &[u8]) -> String {
    node.utf8_text(source).unwrap_or_default().to_string()
}

fn bounded_node_text(node: Node<'_>, source: &[u8]) -> String {
    let value = node_text(node, source);
    if value.chars().count() <= MAX_TEXT_FIELD_CHARS {
        return value;
    }
    value.chars().take(MAX_TEXT_FIELD_CHARS).collect()
}

pub(super) fn source_range(node: Node<'_>) -> SourceRange {
    SourceRange {
        start: SourcePoint {
            byte: node.start_byte(),
            row: node.start_position().row,
            column: node.start_position().column,
        },
        end: SourcePoint {
            byte: node.end_byte(),
            row: node.end_position().row,
            column: node.end_position().column,
        },
    }
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}
