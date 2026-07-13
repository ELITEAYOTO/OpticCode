use std::collections::BTreeMap;
use std::path::Path;

use crate::java_syntax::{JavaReference, JavaReferenceKind};

use super::declarations::owner_id;
use super::imports::JavaFileContext;
use super::schema::{
    JavaCandidateOrigin, JavaIndexedReference, JavaIndexedSymbol, JavaIndexedSymbolKind,
    JavaReferenceResolution, JavaResolutionCandidate, JavaResolutionStatus,
};

const JAVA_LANG_TYPES: &[&str] = &[
    "Appendable",
    "AutoCloseable",
    "Boolean",
    "Byte",
    "Character",
    "CharSequence",
    "Class",
    "ClassLoader",
    "Cloneable",
    "Comparable",
    "Deprecated",
    "Double",
    "Enum",
    "Error",
    "Exception",
    "Float",
    "IllegalArgumentException",
    "Integer",
    "Iterable",
    "Long",
    "Math",
    "Number",
    "Object",
    "Override",
    "Process",
    "ProcessBuilder",
    "Runnable",
    "RuntimeException",
    "Short",
    "String",
    "StringBuffer",
    "StringBuilder",
    "SuppressWarnings",
    "System",
    "Thread",
    "Throwable",
    "Void",
];

pub(super) struct JavaResolver<'symbols> {
    symbols: &'symbols [JavaIndexedSymbol],
    types_by_id: BTreeMap<String, Vec<usize>>,
    types_by_simple_name: BTreeMap<String, Vec<usize>>,
    members_by_owner_and_name: BTreeMap<String, BTreeMap<String, Vec<usize>>>,
    members_by_name: BTreeMap<String, Vec<usize>>,
    candidate_limit: usize,
}

impl<'symbols> JavaResolver<'symbols> {
    pub fn new(symbols: &'symbols [JavaIndexedSymbol], candidate_limit: usize) -> Self {
        let mut resolver = Self {
            symbols,
            types_by_id: BTreeMap::new(),
            types_by_simple_name: BTreeMap::new(),
            members_by_owner_and_name: BTreeMap::new(),
            members_by_name: BTreeMap::new(),
            candidate_limit,
        };
        for (index, symbol) in symbols.iter().enumerate() {
            if symbol.kind.is_type() {
                resolver
                    .types_by_id
                    .entry(symbol.id.clone())
                    .or_default()
                    .push(index);
                resolver
                    .types_by_simple_name
                    .entry(symbol.name.clone())
                    .or_default()
                    .push(index);
            } else if let Some(owner) = &symbol.owner_id {
                resolver
                    .members_by_owner_and_name
                    .entry(owner.clone())
                    .or_default()
                    .entry(symbol.name.clone())
                    .or_default()
                    .push(index);
                resolver
                    .members_by_name
                    .entry(symbol.name.clone())
                    .or_default()
                    .push(index);
            }
        }
        resolver
    }

    pub fn index_reference(
        &self,
        context: &JavaFileContext,
        reference: &JavaReference,
    ) -> JavaIndexedReference {
        let reference_owner = owner_id(context.package.as_deref(), reference.container.as_deref());
        let resolution = if context.syntax_valid {
            self.resolve_reference(context, reference, reference_owner.as_deref())
        } else {
            JavaReferenceResolution {
                status: JavaResolutionStatus::InvalidSyntaxContext,
                target_id: None,
                reason: "the containing file has Tree-sitter ERROR or MISSING diagnostics"
                    .to_string(),
                candidates: Vec::new(),
                candidates_truncated: false,
            }
        };
        JavaIndexedReference {
            id: format!(
                "{}:{}:{}:{}",
                normalized_path(&context.path),
                reference.range.start.byte,
                reference.range.end.byte,
                reference_kind_name(reference.kind)
            ),
            kind: reference.kind,
            name: reference.name.clone(),
            qualifier: reference.qualifier.clone(),
            owner_id: reference_owner,
            argument_count: reference.argument_count,
            file: context.path.clone(),
            source_hash: context.source_hash.clone(),
            range: reference.range,
            name_range: reference.name_range,
            resolution,
        }
    }

    fn resolve_reference(
        &self,
        context: &JavaFileContext,
        reference: &JavaReference,
        reference_owner: Option<&str>,
    ) -> JavaReferenceResolution {
        match reference.kind {
            JavaReferenceKind::TypeUsage | JavaReferenceKind::Annotation => {
                let type_name = qualified_reference_name(reference);
                self.resolve_type(context, reference_owner, &type_name)
            }
            JavaReferenceKind::ConstructorCall => {
                self.resolve_constructor(context, reference_owner, reference)
            }
            JavaReferenceKind::MethodInvocation | JavaReferenceKind::MethodReference => {
                self.resolve_method(context, reference_owner, reference)
            }
            JavaReferenceKind::FieldAccess => {
                self.resolve_field(context, reference_owner, reference)
            }
        }
    }

    fn resolve_type(
        &self,
        context: &JavaFileContext,
        reference_owner: Option<&str>,
        raw_name: &str,
    ) -> JavaReferenceResolution {
        let name = normalize_type_name(raw_name);
        if name.is_empty() {
            return unresolved("the type reference is empty after normalization");
        }

        for owner in owner_chain(reference_owner, context.package.as_deref()) {
            let candidate_id = format!("{owner}.{name}");
            if let Some(indices) = self.types_by_id.get(&candidate_id) {
                return self.finish(
                    self.indexed_candidates(indices, JavaCandidateOrigin::LocalOrNested),
                    JavaResolutionStatus::Exact,
                    "type resolved in the current or enclosing type",
                    "multiple local or nested declarations match the type",
                );
            }
        }

        if !looks_fully_qualified(&name) {
            let candidate_id = context
                .package
                .as_deref()
                .filter(|value| !value.is_empty())
                .map_or_else(|| name.clone(), |package| format!("{package}.{name}"));
            if let Some(indices) = self.types_by_id.get(&candidate_id) {
                return self.finish(
                    self.indexed_candidates(indices, JavaCandidateOrigin::SamePackage),
                    JavaResolutionStatus::Exact,
                    "type resolved from the same package",
                    "multiple same-package declarations match the type",
                );
            }
        }

        let (first_segment, suffix) = name.split_once('.').unwrap_or((&name, ""));
        if let Some(imports) = context.explicit_types.get(first_segment) {
            let candidates = imports
                .iter()
                .flat_map(|imported| {
                    let id = if suffix.is_empty() {
                        imported.clone()
                    } else {
                        format!("{imported}.{suffix}")
                    };
                    self.type_candidates_or_external(&id, JavaCandidateOrigin::ExplicitImport)
                })
                .take(self.candidate_probe_limit())
                .collect();
            return self.finish(
                candidates,
                JavaResolutionStatus::Exact,
                "type resolved through an explicit import",
                "conflicting explicit imports match the type",
            );
        }

        if looks_fully_qualified(&name) {
            if let Some(indices) = self.types_by_id.get(&name) {
                return self.finish(
                    self.indexed_candidates(indices, JavaCandidateOrigin::FullyQualified),
                    JavaResolutionStatus::Exact,
                    "fully qualified type declaration found in the index",
                    "multiple indexed declarations share the fully qualified type name",
                );
            }
            return self.finish(
                vec![external_candidate(
                    name,
                    JavaCandidateOrigin::FullyQualified,
                )],
                JavaResolutionStatus::Exact,
                "the source uses a fully qualified type name; its declaration is external",
                "multiple fully qualified candidates match the type",
            );
        }

        if !name.contains('.') {
            let mut on_demand = Vec::new();
            for package in &context.wildcard_packages {
                let id = format!("{package}.{name}");
                if let Some(indices) = self.types_by_id.get(&id) {
                    on_demand.extend(
                        self.indexed_candidates(indices, JavaCandidateOrigin::WildcardImport),
                    );
                    if on_demand.len() >= self.candidate_probe_limit() {
                        on_demand.truncate(self.candidate_probe_limit());
                        break;
                    }
                }
            }
            if on_demand.len() < self.candidate_probe_limit()
                && JAVA_LANG_TYPES.binary_search(&name.as_str()).is_ok()
            {
                on_demand.push(external_candidate(
                    format!("java.lang.{name}"),
                    JavaCandidateOrigin::JavaLang,
                ));
            }
            if !on_demand.is_empty() {
                let only_java_lang =
                    on_demand.len() == 1 && on_demand[0].origin == JavaCandidateOrigin::JavaLang;
                return self.finish(
                    on_demand,
                    if only_java_lang {
                        JavaResolutionStatus::Exact
                    } else {
                        JavaResolutionStatus::UniqueCandidate
                    },
                    if only_java_lang {
                        "type resolved from the conservative java.lang allowlist"
                    } else {
                        "one indexed on-demand import candidate matches the type"
                    },
                    "multiple on-demand imports, including java.lang, match the type",
                );
            }

            if let Some(indices) = self.types_by_simple_name.get(&name) {
                return self.finish(
                    self.indexed_candidates(indices, JavaCandidateOrigin::GlobalIndex),
                    JavaResolutionStatus::UniqueCandidate,
                    "one global indexed declaration has this simple type name",
                    "multiple global declarations have this simple type name",
                );
            }
        }

        unresolved("no indexed declaration or syntactically proven external type matches")
    }

    fn resolve_constructor(
        &self,
        context: &JavaFileContext,
        reference_owner: Option<&str>,
        reference: &JavaReference,
    ) -> JavaReferenceResolution {
        let type_name = qualified_reference_name(reference);
        let type_resolution = self.resolve_type(context, reference_owner, &type_name);
        let Some(owner) = type_resolution.target_id.as_deref() else {
            return type_resolution;
        };
        let constructors = self.member_candidates(
            owner,
            &simple_name(owner),
            JavaIndexedSymbolKind::Constructor,
            reference.argument_count,
        );
        if constructors.is_empty() {
            if self.has_member_kind(owner, JavaIndexedSymbolKind::Constructor) {
                return unresolved(
                    "constructor owner resolved but no indexed overload has a compatible arity",
                );
            }
            let owner_is_external = type_resolution
                .candidates
                .first()
                .is_some_and(|candidate| candidate.external);
            if owner_is_external {
                return JavaReferenceResolution {
                    status: JavaResolutionStatus::UniqueCandidate,
                    reason: "constructor type is exact but the external overload is not indexed"
                        .to_string(),
                    ..type_resolution
                };
            }
            if reference.argument_count != Some(0) {
                return unresolved("indexed type has only an implicit zero-argument constructor");
            }
            return JavaReferenceResolution {
                reason: "indexed type has an implicit zero-argument constructor".to_string(),
                ..type_resolution
            };
        }
        let unique_status = if type_resolution.status == JavaResolutionStatus::Exact {
            JavaResolutionStatus::Exact
        } else {
            JavaResolutionStatus::UniqueCandidate
        };
        self.finish(
            constructors,
            unique_status,
            "constructor owner and arity match one indexed declaration",
            "multiple constructor overloads remain compatible with the call",
        )
    }

    fn resolve_method(
        &self,
        context: &JavaFileContext,
        reference_owner: Option<&str>,
        reference: &JavaReference,
    ) -> JavaReferenceResolution {
        if let Some(qualifier) = reference.qualifier.as_deref() {
            let owner_resolution = self.resolve_type(context, reference_owner, qualifier);
            if let Some(owner) = owner_resolution.target_id.as_deref() {
                let members = self.static_member_candidates(
                    owner,
                    &reference.name,
                    JavaIndexedSymbolKind::Method,
                    reference.argument_count,
                    JavaCandidateOrigin::OwnerMember,
                );
                if !members.is_empty() {
                    let unique_status = if owner_resolution.status == JavaResolutionStatus::Exact {
                        JavaResolutionStatus::Exact
                    } else {
                        JavaResolutionStatus::UniqueCandidate
                    };
                    return self.finish(
                        members,
                        unique_status,
                        "qualified method owner and arity match an indexed declaration",
                        "multiple method overloads remain compatible with the qualified call",
                    );
                }
                if owner_resolution
                    .candidates
                    .first()
                    .is_some_and(|candidate| candidate.external)
                {
                    return self.finish(
                        vec![external_candidate(
                            format!("{owner}#{}", reference.name),
                            JavaCandidateOrigin::OwnerMember,
                        )],
                        JavaResolutionStatus::UniqueCandidate,
                        "the owner is exact but the external method overload is not indexed",
                        "multiple external method candidates match",
                    );
                }
                if owner_resolution.status == JavaResolutionStatus::Exact {
                    return unresolved(
                        "qualified owner resolved but no indexed method has a compatible arity",
                    );
                }
            }
        } else {
            for owner in owner_chain(reference_owner, context.package.as_deref()) {
                let members = self.member_candidates(
                    &owner,
                    &reference.name,
                    JavaIndexedSymbolKind::Method,
                    reference.argument_count,
                );
                if !members.is_empty() {
                    return self.finish(
                        members,
                        JavaResolutionStatus::Exact,
                        "unqualified call resolved in the current or enclosing type",
                        "multiple local overloads remain compatible with the call",
                    );
                }
            }

            if let Some(owners) = context.explicit_static_members.get(&reference.name) {
                let candidates = self.static_method_candidates(
                    owners,
                    &reference.name,
                    reference.argument_count,
                    JavaCandidateOrigin::StaticExplicitImport,
                    true,
                );
                let unique_status = if candidates.iter().all(|candidate| candidate.external) {
                    JavaResolutionStatus::UniqueCandidate
                } else {
                    JavaResolutionStatus::Exact
                };
                return self.finish(
                    candidates,
                    unique_status,
                    if unique_status == JavaResolutionStatus::Exact {
                        "method resolved through an explicit static import"
                    } else {
                        "the explicit static import proves the owner and member but not the external overload"
                    },
                    "multiple explicit static imports or overloads match the call",
                );
            }

            let wildcard_candidates = self.static_method_candidates(
                &context.static_wildcard_owners,
                &reference.name,
                reference.argument_count,
                JavaCandidateOrigin::StaticWildcardImport,
                false,
            );
            if !wildcard_candidates.is_empty() {
                return self.finish(
                    wildcard_candidates,
                    JavaResolutionStatus::UniqueCandidate,
                    "one indexed static wildcard candidate matches the call",
                    "multiple static wildcard candidates match the call",
                );
            }
        }

        self.resolve_global_member(
            &reference.name,
            JavaIndexedSymbolKind::Method,
            reference.argument_count,
            "one global method candidate matches without enough owner information",
            "multiple global methods match without enough owner information",
        )
    }

    fn resolve_field(
        &self,
        context: &JavaFileContext,
        reference_owner: Option<&str>,
        reference: &JavaReference,
    ) -> JavaReferenceResolution {
        if let Some(qualifier) = reference.qualifier.as_deref() {
            let owner_resolution = self.resolve_type(context, reference_owner, qualifier);
            if let Some(owner) = owner_resolution.target_id.as_deref() {
                let mut members = self.static_member_candidates(
                    owner,
                    &reference.name,
                    JavaIndexedSymbolKind::Field,
                    None,
                    JavaCandidateOrigin::OwnerMember,
                );
                members.extend(self.static_member_candidates(
                    owner,
                    &reference.name,
                    JavaIndexedSymbolKind::EnumConstant,
                    None,
                    JavaCandidateOrigin::OwnerMember,
                ));
                self.bound_candidate_probe(&mut members);
                if !members.is_empty() {
                    let unique_status = if owner_resolution.status == JavaResolutionStatus::Exact {
                        JavaResolutionStatus::Exact
                    } else {
                        JavaResolutionStatus::UniqueCandidate
                    };
                    return self.finish(
                        members,
                        unique_status,
                        "qualified owner and member match an indexed field or enum constant",
                        "multiple indexed members match the qualified field access",
                    );
                }
                if owner_resolution.status == JavaResolutionStatus::Exact
                    && owner_resolution
                        .candidates
                        .first()
                        .is_some_and(|candidate| candidate.external)
                {
                    return self.finish(
                        vec![external_candidate(
                            format!("{owner}#{}", reference.name),
                            JavaCandidateOrigin::OwnerMember,
                        )],
                        JavaResolutionStatus::Exact,
                        "the explicitly resolved external type proves the qualified member path",
                        "multiple external member paths match",
                    );
                }
                if owner_resolution.status == JavaResolutionStatus::Exact {
                    return unresolved(
                        "qualified owner resolved but no indexed field or enum constant matches",
                    );
                }
            }
        } else if let Some(owner) = reference_owner {
            let mut members =
                self.member_candidates(owner, &reference.name, JavaIndexedSymbolKind::Field, None);
            members.extend(self.member_candidates(
                owner,
                &reference.name,
                JavaIndexedSymbolKind::EnumConstant,
                None,
            ));
            self.bound_candidate_probe(&mut members);
            if !members.is_empty() {
                return self.finish(
                    members,
                    JavaResolutionStatus::Exact,
                    "field resolved in the current type",
                    "multiple fields in the current type match",
                );
            }
        }

        let mut global =
            self.global_member_candidates(&reference.name, JavaIndexedSymbolKind::Field, None);
        global.extend(self.global_member_candidates(
            &reference.name,
            JavaIndexedSymbolKind::EnumConstant,
            None,
        ));
        self.bound_candidate_probe(&mut global);
        if global.is_empty() {
            unresolved("field owner could not be proven and no indexed member matches")
        } else {
            self.finish(
                global,
                JavaResolutionStatus::UniqueCandidate,
                "one global field or enum constant candidate matches",
                "multiple global fields or enum constants match",
            )
        }
    }

    fn resolve_global_member(
        &self,
        name: &str,
        kind: JavaIndexedSymbolKind,
        arity: Option<usize>,
        unique_reason: &str,
        ambiguous_reason: &str,
    ) -> JavaReferenceResolution {
        let candidates = self.global_member_candidates(name, kind, arity);
        if candidates.is_empty() {
            unresolved("member owner could not be proven and no indexed declaration matches")
        } else {
            self.finish(
                candidates,
                JavaResolutionStatus::UniqueCandidate,
                unique_reason,
                ambiguous_reason,
            )
        }
    }

    fn member_candidates(
        &self,
        owner: &str,
        name: &str,
        kind: JavaIndexedSymbolKind,
        arity: Option<usize>,
    ) -> Vec<JavaResolutionCandidate> {
        let indices = self
            .members_by_owner_and_name
            .get(owner)
            .and_then(|members| members.get(name))
            .map(Vec::as_slice)
            .unwrap_or_default();
        self.filtered_member_candidates(
            indices,
            kind,
            arity,
            JavaCandidateOrigin::OwnerMember,
            false,
        )
    }

    fn has_member_kind(&self, owner: &str, kind: JavaIndexedSymbolKind) -> bool {
        self.members_by_owner_and_name
            .get(owner)
            .into_iter()
            .flat_map(|members| members.values())
            .flatten()
            .any(|index| self.symbols[*index].kind == kind)
    }

    fn global_member_candidates(
        &self,
        name: &str,
        kind: JavaIndexedSymbolKind,
        arity: Option<usize>,
    ) -> Vec<JavaResolutionCandidate> {
        let indices = self
            .members_by_name
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        self.filtered_member_candidates(
            indices,
            kind,
            arity,
            JavaCandidateOrigin::GlobalIndex,
            false,
        )
    }

    fn static_member_candidates(
        &self,
        owner: &str,
        name: &str,
        kind: JavaIndexedSymbolKind,
        arity: Option<usize>,
        origin: JavaCandidateOrigin,
    ) -> Vec<JavaResolutionCandidate> {
        let indices = self
            .members_by_owner_and_name
            .get(owner)
            .and_then(|members| members.get(name))
            .map(Vec::as_slice)
            .unwrap_or_default();
        self.filtered_member_candidates(indices, kind, arity, origin, true)
    }

    fn filtered_member_candidates(
        &self,
        indices: &[usize],
        kind: JavaIndexedSymbolKind,
        arity: Option<usize>,
        origin: JavaCandidateOrigin,
        require_static: bool,
    ) -> Vec<JavaResolutionCandidate> {
        if let Some(arity) = arity {
            let matching_arity = indices
                .iter()
                .copied()
                .filter(|index| {
                    let symbol = &self.symbols[*index];
                    symbol.kind == kind
                        && (!require_static || symbol.is_static)
                        && symbol.parameter_count == Some(arity)
                })
                .take(self.candidate_limit.saturating_add(1))
                .collect::<Vec<_>>();
            return self.indexed_candidates(&matching_arity, origin);
        }
        let matching_kind = indices
            .iter()
            .copied()
            .filter(|index| {
                let symbol = &self.symbols[*index];
                symbol.kind == kind && (!require_static || symbol.is_static)
            })
            .take(self.candidate_limit.saturating_add(1))
            .collect::<Vec<_>>();
        self.indexed_candidates(&matching_kind, origin)
    }

    fn static_method_candidates(
        &self,
        owners: &[String],
        name: &str,
        arity: Option<usize>,
        origin: JavaCandidateOrigin,
        include_external: bool,
    ) -> Vec<JavaResolutionCandidate> {
        owners
            .iter()
            .flat_map(|owner| {
                let mut candidates = self.static_member_candidates(
                    owner,
                    name,
                    JavaIndexedSymbolKind::Method,
                    arity,
                    origin,
                );
                if candidates.is_empty()
                    && include_external
                    && !self.types_by_id.contains_key(owner)
                {
                    candidates.push(external_candidate(format!("{owner}#{name}"), origin));
                }
                candidates
            })
            .take(self.candidate_probe_limit())
            .collect()
    }

    fn type_candidates_or_external(
        &self,
        id: &str,
        origin: JavaCandidateOrigin,
    ) -> Vec<JavaResolutionCandidate> {
        self.types_by_id.get(id).map_or_else(
            || vec![external_candidate(id.to_string(), origin)],
            |indices| self.indexed_candidates(indices, origin),
        )
    }

    fn indexed_candidates(
        &self,
        indices: &[usize],
        origin: JavaCandidateOrigin,
    ) -> Vec<JavaResolutionCandidate> {
        indices
            .iter()
            .take(self.candidate_probe_limit())
            .map(|index| {
                let symbol = &self.symbols[*index];
                JavaResolutionCandidate {
                    symbol_id: symbol.id.clone(),
                    origin,
                    external: false,
                    file: Some(symbol.file.clone()),
                    range: Some(symbol.range),
                }
            })
            .collect()
    }

    fn candidate_probe_limit(&self) -> usize {
        self.candidate_limit.saturating_add(1)
    }

    fn bound_candidate_probe(&self, candidates: &mut Vec<JavaResolutionCandidate>) {
        candidates.truncate(self.candidate_probe_limit());
    }

    fn finish(
        &self,
        mut candidates: Vec<JavaResolutionCandidate>,
        unique_status: JavaResolutionStatus,
        unique_reason: &str,
        ambiguous_reason: &str,
    ) -> JavaReferenceResolution {
        candidates.sort_by(|left, right| {
            left.symbol_id
                .cmp(&right.symbol_id)
                .then_with(|| left.origin.cmp(&right.origin))
                .then_with(|| left.file.cmp(&right.file))
                .then_with(|| {
                    left.range
                        .map(|range| range.start.byte)
                        .cmp(&right.range.map(|range| range.start.byte))
                })
        });
        candidates.dedup_by(|left, right| {
            left.symbol_id == right.symbol_id
                && left.file == right.file
                && left.range == right.range
        });
        let candidate_count = candidates.len();
        let status = match candidate_count {
            0 => JavaResolutionStatus::Unresolved,
            1 => unique_status,
            _ => JavaResolutionStatus::Ambiguous,
        };
        let target_id = (candidate_count == 1).then(|| candidates[0].symbol_id.clone());
        let candidates_truncated = candidate_count > self.candidate_limit;
        candidates.truncate(self.candidate_limit);
        JavaReferenceResolution {
            status,
            target_id,
            reason: if candidate_count == 1 {
                unique_reason
            } else if candidate_count > 1 {
                ambiguous_reason
            } else {
                "no candidate matched"
            }
            .to_string(),
            candidates,
            candidates_truncated,
        }
    }
}

fn unresolved(reason: &str) -> JavaReferenceResolution {
    JavaReferenceResolution {
        status: JavaResolutionStatus::Unresolved,
        target_id: None,
        reason: reason.to_string(),
        candidates: Vec::new(),
        candidates_truncated: false,
    }
}

fn external_candidate(
    symbol_id: impl Into<String>,
    origin: JavaCandidateOrigin,
) -> JavaResolutionCandidate {
    JavaResolutionCandidate {
        symbol_id: symbol_id.into(),
        origin,
        external: true,
        file: None,
        range: None,
    }
}

fn qualified_reference_name(reference: &JavaReference) -> String {
    match reference.qualifier.as_deref() {
        Some(qualifier) if !reference.name.starts_with(qualifier) => {
            format!("{qualifier}.{}", reference.name)
        }
        _ => reference.name.clone(),
    }
}

fn normalize_type_name(value: &str) -> String {
    let mut normalized = String::new();
    let mut generic_depth = 0usize;
    for character in value.trim().chars() {
        match character {
            '<' => generic_depth = generic_depth.saturating_add(1),
            '>' => generic_depth = generic_depth.saturating_sub(1),
            '[' | ']' if generic_depth == 0 => {}
            _ if generic_depth == 0 && !character.is_whitespace() => normalized.push(character),
            _ => {}
        }
    }
    normalized.trim_end_matches("...").to_string()
}

fn owner_chain(owner: Option<&str>, package: Option<&str>) -> Vec<String> {
    let Some(owner) = owner else {
        return Vec::new();
    };
    let mut chain = Vec::new();
    let mut current = owner.to_string();
    let package = package.unwrap_or_default();
    while !current.is_empty() && current != package {
        chain.push(current.clone());
        let Some((parent, _)) = current.rsplit_once('.') else {
            break;
        };
        current = parent.to_string();
    }
    chain
}

fn looks_fully_qualified(name: &str) -> bool {
    name.split_once('.').is_some_and(|(first, _)| {
        first
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_lowercase())
    })
}

fn simple_name(value: &str) -> String {
    value.rsplit('.').next().unwrap_or(value).to_string()
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn reference_kind_name(kind: JavaReferenceKind) -> &'static str {
    match kind {
        JavaReferenceKind::TypeUsage => "type_usage",
        JavaReferenceKind::MethodInvocation => "method_invocation",
        JavaReferenceKind::FieldAccess => "field_access",
        JavaReferenceKind::ConstructorCall => "constructor_call",
        JavaReferenceKind::MethodReference => "method_reference",
        JavaReferenceKind::Annotation => "annotation",
    }
}
