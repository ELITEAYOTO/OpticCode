use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use tree_sitter::Parser;

use super::symbols::collect_file_report;
use super::JavaSyntaxFileReport;

pub(super) struct JavaSyntaxParser {
    parser: Parser,
}

impl JavaSyntaxParser {
    pub(super) fn new() -> Result<Self> {
        let mut parser = Parser::new();
        let language = tree_sitter_java::LANGUAGE.into();
        parser
            .set_language(&language)
            .context("failed to load the Tree-sitter Java grammar")?;
        Ok(Self { parser })
    }

    pub(super) fn parse(
        &mut self,
        path: PathBuf,
        source: &str,
        item_limit: usize,
    ) -> Result<JavaSyntaxFileReport> {
        let parse_started = Instant::now();
        let tree = self
            .parser
            .parse(source, None)
            .context("Tree-sitter Java parser returned no syntax tree")?;
        let parse_duration = parse_started.elapsed();
        collect_file_report(path, source, &tree, parse_duration, item_limit)
    }
}
