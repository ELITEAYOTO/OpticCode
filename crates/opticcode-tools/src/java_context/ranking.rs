use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use crate::java_index::{
    JavaIndexProjectReport, JavaIndexedReference, JavaIndexedSymbol, JavaResolutionStatus,
};
use crate::java_syntax::SourceRange;

use super::query::{searchable_terms, ParsedQuery};
use super::schema::{JavaContextCandidate, JavaContextMatchKind, JavaContextScoreReason};
use super::{JAVA_CONTEXT_CANDIDATE_REASON_LIMIT, JAVA_CONTEXT_MIN_CANDIDATE_SCORE};

pub(super) struct RankingResult {
    pub candidates: Vec<JavaContextCandidate>,
    pub scored_candidates: usize,
    pub eligible_candidates: usize,
    pub candidates_truncated: bool,
    pub primary_symbol: Option<String>,
    pub primary_symbols: Vec<String>,
    pub primary_ambiguous: bool,
    pub primary_score_ties: usize,
    pub primary_match_ties: usize,
    pub visited_symbols: usize,
    pub ignored_symbols: usize,
    pub invalid_context_symbols_ignored: usize,
    pub symbols_truncated: bool,
    pub relations_examined: usize,
    pub relations_followed: usize,
    pub ignored_relations: usize,
    pub relation_cycles_skipped: usize,
    pub invalid_context_references_ignored: usize,
    pub relations_truncated: bool,
    pub relation_depth_truncated: bool,
    pub deepest_relation_depth: usize,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RankingLimits {
    pub max_symbols_visited: usize,
    pub max_primary_symbols: usize,
    pub max_candidates: usize,
    pub max_relations: usize,
    pub max_relation_depth: usize,
    pub max_callers_per_symbol: usize,
    pub max_related_symbols: usize,
}

struct CandidateBuilder<'a> {
    symbol: &'a JavaIndexedSymbol,
    score: u32,
    reasons: BTreeMap<(JavaContextMatchKind, String), u32>,
}

impl<'a> CandidateBuilder<'a> {
    fn new(symbol: &'a JavaIndexedSymbol) -> Self {
        Self {
            symbol,
            score: 0,
            reasons: BTreeMap::new(),
        }
    }

    fn add_reason(&mut self, kind: JavaContextMatchKind, score: u32, detail: String) {
        if self.reasons.insert((kind, detail), score).is_none() {
            self.score = self.score.saturating_add(score);
        }
    }

    fn finish(self) -> JavaContextCandidate {
        let mut reasons = self
            .reasons
            .into_iter()
            .map(|((kind, detail), score)| JavaContextScoreReason {
                kind,
                score,
                detail,
            })
            .collect::<Vec<_>>();
        reasons.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.detail.cmp(&right.detail))
        });
        let reason_count = reasons.len();
        let reasons_truncated = reason_count > JAVA_CONTEXT_CANDIDATE_REASON_LIMIT;
        reasons.truncate(JAVA_CONTEXT_CANDIDATE_REASON_LIMIT);
        JavaContextCandidate {
            symbol_id: self.symbol.id.clone(),
            kind: self.symbol.kind,
            name: self.symbol.name.clone(),
            qualified_name: self.symbol.qualified_name.clone(),
            signature: self.symbol.signature.clone(),
            file: self.symbol.file.clone(),
            source_hash: self.symbol.source_hash.clone(),
            range: self.symbol.range,
            name_range: self.symbol.name_range,
            score: self.score,
            reason_count,
            reasons_truncated,
            reasons,
        }
    }
}

pub(super) fn rank_symbols(
    index: &JavaIndexProjectReport,
    query: &ParsedQuery,
    limits: RankingLimits,
) -> RankingResult {
    debug_assert_eq!(limits.max_relation_depth, 1);
    let valid_files = index
        .files
        .iter()
        .filter(|file| file.syntax_valid)
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    let valid_symbol_count = index
        .symbols
        .iter()
        .filter(|symbol| valid_files.contains(&symbol.file))
        .count();
    let symbols = index
        .symbols
        .iter()
        .filter(|symbol| valid_files.contains(&symbol.file))
        .take(limits.max_symbols_visited)
        .collect::<Vec<_>>();
    let visited_symbols = symbols.len();
    let ignored_symbols = valid_symbol_count.saturating_sub(visited_symbols);
    let invalid_context_symbols_ignored = index.symbols.len().saturating_sub(valid_symbol_count);
    let symbols_truncated = ignored_symbols > 0;
    let symbols_by_id = symbols
        .iter()
        .map(|symbol| (symbol.id.as_str(), *symbol))
        .collect::<HashMap<_, _>>();
    let symbols_by_file = symbols_by_file(&symbols);
    let mut builders = BTreeMap::<String, CandidateBuilder<'_>>::new();

    for symbol in &symbols {
        score_symbol(symbol, query, &mut builders);
    }
    let precise_direct_match = builders
        .values()
        .filter_map(direct_match_level)
        .max()
        .is_some_and(|level| level >= 2);
    let (mut direct_primary_symbols, primary_match_ties) = direct_primary_matches(&builders);
    direct_primary_symbols.truncate(limits.max_primary_symbols);
    let mut invalid_context_references_ignored = 0usize;
    for reference in &index.references {
        if reference.resolution.status == JavaResolutionStatus::InvalidSyntaxContext
            || !valid_files.contains(&reference.file)
        {
            invalid_context_references_ignored =
                invalid_context_references_ignored.saturating_add(1);
            continue;
        }
        if precise_direct_match {
            continue;
        }
        if !reference_matches_query(reference, query) {
            continue;
        }
        if reference.resolution.status == JavaResolutionStatus::Exact {
            if let Some(target_id) = reference.resolution.target_id.as_deref() {
                if let Some(symbol) = symbols_by_id.get(target_id).copied() {
                    add_reason(
                        &mut builders,
                        symbol,
                        JavaContextMatchKind::MatchingReferenceTarget,
                        460,
                        format!("query matches reference {}", reference.id),
                    );
                }
            }
        }
        if let Some(owner) = enclosing_symbol(reference, &symbols_by_file) {
            add_reason(
                &mut builders,
                owner,
                JavaContextMatchKind::MatchingReferenceOwner,
                340,
                format!("contains matching reference {}", reference.id),
            );
        }
    }

    let mut seed_scores = builders
        .values()
        .filter(|builder| retain_candidate(builder))
        .map(|builder| (builder.symbol.id.as_str(), builder.score))
        .collect::<Vec<_>>();
    seed_scores.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    let primary_score = seed_scores.first().map(|candidate| candidate.1);
    let primary_score_ties = primary_score.map_or(0, |score| {
        seed_scores
            .iter()
            .take_while(|candidate| candidate.1 == score)
            .count()
    });
    let score_primary_symbols = seed_scores
        .iter()
        .take(primary_score_ties.min(limits.max_primary_symbols))
        .map(|(id, _)| (*id).to_string())
        .collect::<Vec<_>>();
    let primary_symbols = if direct_primary_symbols.is_empty() {
        score_primary_symbols
    } else {
        direct_primary_symbols
    };
    let seed_ids = primary_symbols.iter().cloned().collect::<BTreeSet<_>>();
    let seed_symbols = primary_symbols
        .iter()
        .filter_map(|id| symbols_by_id.get(id.as_str()).copied())
        .collect::<Vec<_>>();

    let mut caller_counts = BTreeMap::<String, usize>::new();
    let mut related_count = 0usize;
    let mut relations_examined = 0usize;
    let mut relations_followed = 0usize;
    let mut ignored_relations = 0usize;
    let mut relation_cycle_edges = BTreeSet::<String>::new();
    let mut relations_truncated = false;
    let mut followed_edges = BTreeSet::<(String, JavaContextMatchKind, String)>::new();
    let mut depth_one_symbols = BTreeSet::<String>::new();
    for reference in &index.references {
        if reference.resolution.status != JavaResolutionStatus::Exact
            || !valid_files.contains(&reference.file)
        {
            continue;
        }
        relations_examined = relations_examined.saturating_add(1);
        let target_id = reference.resolution.target_id.as_deref();
        if let Some(target_id) = target_id.filter(|id| seed_ids.contains(*id)) {
            let count = caller_counts.entry(target_id.to_string()).or_default();
            if let Some(caller) = enclosing_symbol(reference, &symbols_by_file) {
                if caller.id == target_id || seed_ids.contains(&caller.id) {
                    relation_cycle_edges.insert(reference.id.clone());
                } else if *count >= limits.max_callers_per_symbol {
                    ignored_relations = ignored_relations.saturating_add(1);
                    relations_truncated = true;
                } else {
                    let edge = (
                        reference.id.clone(),
                        JavaContextMatchKind::CallerOfPrimary,
                        caller.id.clone(),
                    );
                    if followed_edges.insert(edge) {
                        if relations_followed >= limits.max_relations {
                            ignored_relations = ignored_relations.saturating_add(1);
                            relations_truncated = true;
                        } else {
                            add_reason(
                                &mut builders,
                                caller,
                                JavaContextMatchKind::CallerOfPrimary,
                                260,
                                format!("references {target_id}"),
                            );
                            *count += 1;
                            relations_followed = relations_followed.saturating_add(1);
                            depth_one_symbols.insert(caller.id.clone());
                        }
                    }
                }
            }
        }

        let referenced_by_seed = seed_symbols
            .iter()
            .any(|seed| seed.file == reference.file && contains_range(seed.range, reference.range));
        if !referenced_by_seed {
            continue;
        }
        if let Some(target_id) = target_id {
            if seed_ids.contains(target_id) {
                relation_cycle_edges.insert(reference.id.clone());
                continue;
            }
            if let Some(target) = symbols_by_id.get(target_id).copied() {
                if related_count >= limits.max_related_symbols {
                    ignored_relations = ignored_relations.saturating_add(1);
                    relations_truncated = true;
                    continue;
                }
                let edge = (
                    reference.id.clone(),
                    JavaContextMatchKind::ReferencedByPrimary,
                    target.id.clone(),
                );
                if followed_edges.insert(edge) {
                    if relations_followed >= limits.max_relations {
                        ignored_relations = ignored_relations.saturating_add(1);
                        relations_truncated = true;
                    } else {
                        add_reason(
                            &mut builders,
                            target,
                            JavaContextMatchKind::ReferencedByPrimary,
                            180,
                            format!("referenced inside a primary candidate by {}", reference.id),
                        );
                        related_count += 1;
                        relations_followed = relations_followed.saturating_add(1);
                        depth_one_symbols.insert(target.id.clone());
                    }
                }
            }
        }
    }

    let selected_relation_symbols = seed_ids
        .union(&depth_one_symbols)
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut deeper_relations = BTreeSet::<String>::new();
    for reference in &index.references {
        if reference.resolution.status != JavaResolutionStatus::Exact
            || !valid_files.contains(&reference.file)
        {
            continue;
        }
        let Some(owner) = enclosing_symbol(reference, &symbols_by_file) else {
            continue;
        };
        let Some(target_id) = reference.resolution.target_id.as_deref() else {
            continue;
        };
        let omitted_from_depth_one =
            depth_one_symbols.contains(&owner.id) && !selected_relation_symbols.contains(target_id);
        let omitted_caller =
            depth_one_symbols.contains(target_id) && !selected_relation_symbols.contains(&owner.id);
        if omitted_from_depth_one || omitted_caller {
            deeper_relations.insert(reference.id.clone());
        } else if depth_one_symbols.contains(&owner.id) && seed_ids.contains(target_id) {
            relation_cycle_edges.insert(reference.id.clone());
        }
    }
    let relation_depth_truncated = !deeper_relations.is_empty();
    ignored_relations = ignored_relations.saturating_add(deeper_relations.len());

    let scored_candidates = builders
        .values()
        .filter(|builder| builder.score > 0)
        .count();
    let mut candidates = builders
        .into_values()
        .filter(retain_candidate)
        .map(CandidateBuilder::finish)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.symbol_id.cmp(&right.symbol_id))
    });
    let eligible_candidates = candidates.len();
    let candidates_truncated = candidates.len() > limits.max_candidates;
    candidates.truncate(limits.max_candidates);

    let primary_symbol = primary_symbols.first().cloned();

    RankingResult {
        candidates,
        scored_candidates,
        eligible_candidates,
        candidates_truncated,
        primary_symbol,
        primary_symbols,
        primary_ambiguous: primary_match_ties > 1
            || (primary_match_ties == 0 && primary_score_ties > 1),
        primary_score_ties,
        primary_match_ties,
        visited_symbols,
        ignored_symbols,
        invalid_context_symbols_ignored,
        symbols_truncated,
        relations_examined,
        relations_followed,
        ignored_relations,
        relation_cycles_skipped: relation_cycle_edges.len(),
        invalid_context_references_ignored,
        relations_truncated,
        relation_depth_truncated,
        deepest_relation_depth: usize::from(relations_followed > 0),
    }
}

fn retain_candidate(builder: &CandidateBuilder<'_>) -> bool {
    builder.score >= JAVA_CONTEXT_MIN_CANDIDATE_SCORE
        || builder.reasons.keys().any(|(kind, _)| {
            matches!(
                kind,
                JavaContextMatchKind::CallerOfPrimary
                    | JavaContextMatchKind::ReferencedByPrimary
                    | JavaContextMatchKind::MatchingReferenceOwner
                    | JavaContextMatchKind::MatchingReferenceTarget
            )
        })
}

fn score_symbol<'a>(
    symbol: &'a JavaIndexedSymbol,
    query: &ParsedQuery,
    builders: &mut BTreeMap<String, CandidateBuilder<'a>>,
) {
    let id_lower = symbol.id.to_lowercase();
    let qualified_lower = symbol.qualified_name.to_lowercase();
    let name_lower = symbol.name.to_lowercase();
    let signature_lower = symbol.signature.as_deref().map(str::to_lowercase);

    if contains_symbol_token(&query.task_lower, &id_lower) {
        add_reason(
            builders,
            symbol,
            JavaContextMatchKind::ExactSymbolId,
            1_600,
            "task contains the complete symbol id".to_string(),
        );
    }
    if contains_symbol_token(&query.task_lower, &qualified_lower) {
        add_reason(
            builders,
            symbol,
            JavaContextMatchKind::ExactQualifiedName,
            1_200,
            "task contains the qualified name".to_string(),
        );
    }
    if let Some(signature) = signature_lower.as_deref() {
        if contains_symbol_token(&query.task_lower, signature) {
            add_reason(
                builders,
                symbol,
                JavaContextMatchKind::ExactSignature,
                1_000,
                format!("task contains signature {signature}"),
            );
        }
    }

    for identifier in &query.identifiers_lower {
        if identifier == &id_lower {
            add_reason(
                builders,
                symbol,
                JavaContextMatchKind::ExactSymbolId,
                1_400,
                format!("identifier {identifier} equals symbol id"),
            );
        } else if identifier == &qualified_lower {
            add_reason(
                builders,
                symbol,
                JavaContextMatchKind::ExactQualifiedName,
                1_100,
                format!("identifier {identifier} equals qualified name"),
            );
        } else if signature_lower.as_ref() == Some(identifier) {
            add_reason(
                builders,
                symbol,
                JavaContextMatchKind::ExactSignature,
                900,
                format!("identifier {identifier} equals signature"),
            );
        } else if identifier == &name_lower {
            add_reason(
                builders,
                symbol,
                JavaContextMatchKind::ExactName,
                720,
                format!("identifier {identifier} equals symbol name"),
            );
        } else if identifier.ends_with(&format!("#{name_lower}"))
            || identifier.ends_with(&format!(".{name_lower}"))
        {
            add_reason(
                builders,
                symbol,
                JavaContextMatchKind::IdentifierSuffix,
                420,
                format!("identifier {identifier} ends with symbol name"),
            );
        }
    }

    let symbol_terms = searchable_terms(&symbol.name);
    let qualified_terms = searchable_terms(&symbol.qualified_name);
    let file_terms = searchable_terms(&normalized_path(&symbol.file));
    for term in &query.terms_lower {
        if symbol_terms.contains(term) {
            add_reason(
                builders,
                symbol,
                JavaContextMatchKind::SymbolTerm,
                180,
                format!("symbol name matches term {term}"),
            );
        } else if qualified_terms.contains(term) {
            add_reason(
                builders,
                symbol,
                JavaContextMatchKind::QualifiedTerm,
                60,
                format!("qualified name matches term {term}"),
            );
        }
        if file_terms.contains(term) {
            add_reason(
                builders,
                symbol,
                JavaContextMatchKind::FileTerm,
                30,
                format!("file path matches term {term}"),
            );
        }
    }
}

fn direct_primary_matches(
    builders: &BTreeMap<String, CandidateBuilder<'_>>,
) -> (Vec<String>, usize) {
    let best_level = builders.values().filter_map(direct_match_level).max();
    let Some(best_level) = best_level else {
        return (Vec::new(), 0);
    };
    let mut groups = BTreeMap::<(JavaContextMatchKind, String), BTreeSet<String>>::new();
    for builder in builders.values() {
        for (kind, detail) in builder.reasons.keys() {
            if direct_reason_level(*kind) == Some(best_level) {
                groups
                    .entry((*kind, detail.clone()))
                    .or_default()
                    .insert(builder.symbol.id.clone());
            }
        }
    }
    let matches = groups
        .values()
        .flat_map(BTreeSet::iter)
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let largest_ambiguous_group = groups.values().map(BTreeSet::len).max().unwrap_or(0);
    (matches, largest_ambiguous_group)
}

fn direct_match_level(builder: &CandidateBuilder<'_>) -> Option<u8> {
    builder
        .reasons
        .keys()
        .filter_map(|(kind, _)| direct_reason_level(*kind))
        .max()
}

fn direct_reason_level(kind: JavaContextMatchKind) -> Option<u8> {
    match kind {
        JavaContextMatchKind::ExactSymbolId => Some(4),
        JavaContextMatchKind::ExactQualifiedName => Some(3),
        JavaContextMatchKind::ExactSignature => Some(2),
        JavaContextMatchKind::ExactName => Some(1),
        _ => None,
    }
}

fn contains_symbol_token(value: &str, symbol: &str) -> bool {
    if symbol.is_empty() {
        return false;
    }
    value.match_indices(symbol).any(|(start, matched)| {
        let before = value[..start].chars().next_back();
        let end = start + matched.len();
        let after = value[end..].chars().next();
        !before.is_some_and(is_symbol_character) && !after.is_some_and(is_symbol_character)
    })
}

fn is_symbol_character(character: char) -> bool {
    character.is_alphanumeric()
        || matches!(
            character,
            '_' | '$' | '.' | '#' | '(' | ')' | ',' | '[' | ']'
        )
}

fn reference_matches_query(reference: &JavaIndexedReference, query: &ParsedQuery) -> bool {
    let name = reference.name.to_lowercase();
    let qualifier = reference.qualifier.as_deref().map(str::to_lowercase);
    let qualified_member = qualifier.as_ref().map(|owner| format!("{owner}.{name}"));
    query.identifiers_lower.contains(&name)
        || qualifier
            .as_ref()
            .is_some_and(|owner| query.identifiers_lower.contains(owner))
        || qualified_member
            .as_ref()
            .is_some_and(|member| query.identifiers_lower.contains(member))
        || searchable_terms(
            reference
                .name
                .rsplit(['.', '#', '$'])
                .next()
                .unwrap_or(&reference.name),
        )
        .iter()
        .any(|term| query.terms_lower.contains(term))
}

fn add_reason<'a>(
    builders: &mut BTreeMap<String, CandidateBuilder<'a>>,
    symbol: &'a JavaIndexedSymbol,
    kind: JavaContextMatchKind,
    score: u32,
    detail: String,
) {
    builders
        .entry(symbol.id.clone())
        .or_insert_with(|| CandidateBuilder::new(symbol))
        .add_reason(kind, score, detail);
}

fn symbols_by_file<'a>(
    symbols: &[&'a JavaIndexedSymbol],
) -> BTreeMap<PathBuf, Vec<&'a JavaIndexedSymbol>> {
    let mut by_file = BTreeMap::<PathBuf, Vec<&JavaIndexedSymbol>>::new();
    for symbol in symbols {
        by_file
            .entry(symbol.file.clone())
            .or_default()
            .push(*symbol);
    }
    for symbols in by_file.values_mut() {
        symbols.sort_by(|left, right| {
            range_len(left.range)
                .cmp(&range_len(right.range))
                .then_with(|| left.id.cmp(&right.id))
        });
    }
    by_file
}

pub(super) fn enclosing_symbol<'a>(
    reference: &JavaIndexedReference,
    symbols_by_file: &'a BTreeMap<PathBuf, Vec<&'a JavaIndexedSymbol>>,
) -> Option<&'a JavaIndexedSymbol> {
    symbols_by_file
        .get(&reference.file)?
        .iter()
        .copied()
        .find(|symbol| contains_range(symbol.range, reference.range))
}

fn contains_range(outer: SourceRange, inner: SourceRange) -> bool {
    outer.start.byte <= inner.start.byte && outer.end.byte >= inner.end.byte
}

fn range_len(range: SourceRange) -> usize {
    range.end.byte.saturating_sub(range.start.byte)
}

fn normalized_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}
