use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::java_syntax::{
    JavaSymbol, JavaSymbolKind, JavaSyntaxFileReport, JavaSyntaxProjectReport,
};

use super::schema::{
    JavaIndexFile, JavaIndexedPackage, JavaIndexedSymbol, JavaIndexedSymbolKind, JavaVisibility,
};

pub(super) struct DeclarationBuild {
    pub packages: Vec<JavaIndexedPackage>,
    pub files: Vec<JavaIndexFile>,
    pub symbols: Vec<JavaIndexedSymbol>,
    pub source_symbol_count: usize,
    pub symbols_truncated: bool,
}

pub(super) fn build_declarations(
    syntax: &JavaSyntaxProjectReport,
    max_symbols: usize,
) -> DeclarationBuild {
    let mut package_files = BTreeMap::<String, BTreeSet<PathBuf>>::new();
    let mut files = Vec::with_capacity(syntax.files.len());
    let mut symbols = Vec::with_capacity(max_symbols.min(syntax.counts.symbols));
    let mut source_symbol_count = 0usize;

    for file in &syntax.files {
        let package = file.package.as_ref().map(|package| package.name.clone());
        package_files
            .entry(package.clone().unwrap_or_default())
            .or_default()
            .insert(file.path.clone());
        files.push(JavaIndexFile {
            path: file.path.clone(),
            source_hash: file.content_hash.clone(),
            syntax_valid: file.syntax_valid,
            retained_items_truncated: file.retained_items_truncated,
            package: package.clone(),
            imports: file.imports.clone(),
            diagnostic_count: file.counts.diagnostics,
        });

        source_symbol_count = source_symbol_count.saturating_add(file.counts.symbols);
        for symbol in &file.symbols {
            if symbols.len() < max_symbols {
                symbols.push(index_symbol(file, package.as_deref(), symbol));
            }
        }
    }

    files.sort_by_cached_key(|file| normalized_path(&file.path));
    symbols.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| normalized_path(&left.file).cmp(&normalized_path(&right.file)))
            .then_with(|| left.range.start.byte.cmp(&right.range.start.byte))
    });
    let packages = package_files
        .into_iter()
        .map(|(name, files)| JavaIndexedPackage {
            name,
            files: files.into_iter().collect(),
        })
        .collect();

    DeclarationBuild {
        packages,
        files,
        symbols,
        source_symbol_count,
        symbols_truncated: source_symbol_count > max_symbols,
    }
}

fn index_symbol(
    file: &JavaSyntaxFileReport,
    package: Option<&str>,
    symbol: &JavaSymbol,
) -> JavaIndexedSymbol {
    let kind = indexed_kind(symbol.kind);
    let owner_id = owner_id(package, symbol.container.as_deref());
    let (signature, signature_complete) = callable_signature(symbol, file);
    let id = match kind {
        JavaIndexedSymbolKind::Method => format!(
            "{}#{}",
            owner_id
                .as_deref()
                .unwrap_or(symbol.qualified_name.as_str()),
            signature.as_deref().unwrap_or(symbol.name.as_str())
        ),
        JavaIndexedSymbolKind::Constructor => format!(
            "{}#{}",
            owner_id
                .as_deref()
                .unwrap_or(symbol.qualified_name.as_str()),
            signature.as_deref().unwrap_or("<init>(?)")
        ),
        JavaIndexedSymbolKind::Field | JavaIndexedSymbolKind::EnumConstant => format!(
            "{}#{}",
            owner_id
                .as_deref()
                .unwrap_or(symbol.qualified_name.as_str()),
            symbol.name
        ),
        _ => symbol.qualified_name.clone(),
    };

    JavaIndexedSymbol {
        id,
        kind,
        name: symbol.name.clone(),
        qualified_name: symbol.qualified_name.clone(),
        owner_id,
        signature,
        signature_complete,
        parameter_count: matches!(
            symbol.kind,
            JavaSymbolKind::Method | JavaSymbolKind::Constructor
        )
        .then_some(symbol.parameters.len()),
        visibility: visibility(&symbol.modifiers),
        is_static: kind == JavaIndexedSymbolKind::EnumConstant
            || symbol.modifiers.iter().any(|modifier| modifier == "static"),
        annotations: symbol.annotations.clone(),
        file: file.path.clone(),
        source_hash: file.content_hash.clone(),
        range: symbol.range,
        name_range: symbol.name_range,
    }
}

fn callable_signature(symbol: &JavaSymbol, file: &JavaSyntaxFileReport) -> (Option<String>, bool) {
    if !matches!(
        symbol.kind,
        JavaSymbolKind::Method | JavaSymbolKind::Constructor
    ) {
        return (symbol.signature.clone(), true);
    }

    let complete = !file.retained_items_truncated
        && symbol
            .parameters
            .iter()
            .all(|parameter| parameter.value_type.is_some());
    let parameters = symbol
        .parameters
        .iter()
        .map(|parameter| {
            let mut value = parameter
                .value_type
                .as_deref()
                .map(normalize_type)
                .unwrap_or_else(|| "?".to_string());
            if parameter.variadic {
                value.push_str("...");
            }
            value
        })
        .collect::<Vec<_>>()
        .join(",");
    let name = if symbol.kind == JavaSymbolKind::Constructor {
        "<init>"
    } else {
        symbol.name.as_str()
    };
    (Some(format!("{name}({parameters})")), complete)
}

fn normalize_type(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

pub(super) fn owner_id(package: Option<&str>, container: Option<&str>) -> Option<String> {
    match (package.filter(|value| !value.is_empty()), container) {
        (Some(package), Some(container)) => Some(format!("{package}.{container}")),
        (Some(package), None) => Some(package.to_string()),
        (None, Some(container)) => Some(container.to_string()),
        (None, None) => None,
    }
}

fn visibility(modifiers: &[String]) -> JavaVisibility {
    if modifiers.iter().any(|modifier| modifier == "public") {
        JavaVisibility::Public
    } else if modifiers.iter().any(|modifier| modifier == "protected") {
        JavaVisibility::Protected
    } else if modifiers.iter().any(|modifier| modifier == "private") {
        JavaVisibility::Private
    } else {
        JavaVisibility::Default
    }
}

fn indexed_kind(kind: JavaSymbolKind) -> JavaIndexedSymbolKind {
    match kind {
        JavaSymbolKind::Class => JavaIndexedSymbolKind::Class,
        JavaSymbolKind::Interface => JavaIndexedSymbolKind::Interface,
        JavaSymbolKind::Enum => JavaIndexedSymbolKind::Enum,
        JavaSymbolKind::AnnotationType => JavaIndexedSymbolKind::AnnotationType,
        JavaSymbolKind::Record => JavaIndexedSymbolKind::Record,
        JavaSymbolKind::Method => JavaIndexedSymbolKind::Method,
        JavaSymbolKind::Constructor => JavaIndexedSymbolKind::Constructor,
        JavaSymbolKind::Field => JavaIndexedSymbolKind::Field,
        JavaSymbolKind::EnumConstant => JavaIndexedSymbolKind::EnumConstant,
    }
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
