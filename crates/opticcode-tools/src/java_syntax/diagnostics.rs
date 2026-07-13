use tree_sitter::Node;

use super::symbols::source_range;
use super::{JavaDiagnostic, JavaDiagnosticKind};

pub(super) fn diagnostic_for_node(node: Node<'_>) -> Option<JavaDiagnostic> {
    let (kind, message) = if node.is_error() {
        (
            JavaDiagnosticKind::SyntaxError,
            format!("unexpected or invalid Java syntax near `{}`", node.kind()),
        )
    } else if node.is_missing() {
        (
            JavaDiagnosticKind::MissingNode,
            format!("missing Java syntax node `{}`", node.kind()),
        )
    } else {
        return None;
    };

    Some(JavaDiagnostic {
        kind,
        message,
        node_kind: node.kind().to_string(),
        range: source_range(node),
    })
}
