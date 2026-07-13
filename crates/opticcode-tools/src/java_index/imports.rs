use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::java_syntax::JavaImport;

use super::schema::JavaIndexFile;

#[derive(Debug, Clone)]
pub(super) struct JavaFileContext {
    pub path: PathBuf,
    pub package: Option<String>,
    pub syntax_valid: bool,
    pub source_hash: String,
    pub explicit_types: BTreeMap<String, Vec<String>>,
    pub wildcard_packages: Vec<String>,
    pub explicit_static_members: BTreeMap<String, Vec<String>>,
    pub static_wildcard_owners: Vec<String>,
}

impl JavaFileContext {
    pub fn from_file(file: &JavaIndexFile) -> Self {
        let mut context = Self {
            path: file.path.clone(),
            package: file.package.clone(),
            syntax_valid: file.syntax_valid,
            source_hash: file.source_hash.clone(),
            explicit_types: BTreeMap::new(),
            wildcard_packages: Vec::new(),
            explicit_static_members: BTreeMap::new(),
            static_wildcard_owners: Vec::new(),
        };
        for import in &file.imports {
            context.add_import(import);
        }
        sort_and_dedup_map(&mut context.explicit_types);
        sort_and_dedup_map(&mut context.explicit_static_members);
        context.wildcard_packages.sort();
        context.wildcard_packages.dedup();
        context.static_wildcard_owners.sort();
        context.static_wildcard_owners.dedup();
        context
    }

    fn add_import(&mut self, import: &JavaImport) {
        let path = import.path.trim_end_matches(".*");
        if import.is_static && import.wildcard {
            self.static_wildcard_owners.push(path.to_string());
            return;
        }
        if import.is_static {
            if let Some((owner, member)) = path.rsplit_once('.') {
                self.explicit_static_members
                    .entry(member.to_string())
                    .or_default()
                    .push(owner.to_string());
            }
            return;
        }
        if import.wildcard {
            self.wildcard_packages.push(path.to_string());
            return;
        }
        if let Some(simple_name) = path.rsplit('.').next() {
            self.explicit_types
                .entry(simple_name.to_string())
                .or_default()
                .push(path.to_string());
        }
    }
}

pub(super) fn build_file_contexts(files: &[JavaIndexFile]) -> BTreeMap<PathBuf, JavaFileContext> {
    files
        .iter()
        .map(|file| (file.path.clone(), JavaFileContext::from_file(file)))
        .collect()
}

fn sort_and_dedup_map(values: &mut BTreeMap<String, Vec<String>>) {
    for values in values.values_mut() {
        values.sort();
        values.dedup();
    }
}
