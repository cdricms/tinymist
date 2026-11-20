//! Completion by typst specific semantics, like `font`, `package`, `label`, or
//! `typst::foundations::Value`.

use tinymist_std::time::yyyy_mm_dd;
use typst::foundations::Symbol;
use typst::syntax::is_valid_label_literal_id;

use super::*;
use crate::analysis::{LabelDefinitionParams, LabelSiteKind, label_site_kind, label_sites};
impl CompletionPair<'_, '_, '_> {
    /// Add completions for all font families.
    pub fn font_completions(&mut self) {
        let equation = self.cursor.before_window(25).contains("equation");
        let world = self.worker.world().clone();
        let book = world.book();
        for (family, iter) in book.families() {
            let detail = summarize_font_family(iter.filter_map(|id| book.info(id)));
            if !equation || family.contains("Math") {
                self.value_completion(
                    None,
                    &Value::Str(family.into()),
                    false,
                    Some(detail.as_str()),
                );
            }
        }
    }

    /// Add completions for current font features.
    pub fn font_feature_completions(&mut self) {
        // todo: add me
    }

    /// Add completions for all available packages.
    pub fn package_completions(&mut self, all_versions: bool) {
        let w = self.worker.world().clone();
        // Finds packages that are in `@preview`
        let mut packages = w.packages().iter().collect_vec();

        // Finds packages that are not in `@preview`
        #[cfg(feature = "local-registry")]
        let other_packages_refs = self.worker.ctx.non_preview_packages();
        #[cfg(feature = "local-registry")]
        packages.extend(other_packages_refs.iter());

        packages.sort_by_key(|entry| {
            (
                &entry.namespace,
                &entry.package.name,
                Reverse(entry.package.version),
            )
        });
        if !all_versions {
            packages.dedup_by_key(|entry| (&entry.namespace, &entry.package.name));
        }
        let completion_info = packages
            .iter()
            .map(|entry| {
                let mut docs = String::new();
                if let Some(desc) = &entry.package.description {
                    docs.push_str(desc);
                    docs.push_str("\n\n");
                }
                docs.push_str(&format!("Authors: {}\n", entry.package.authors.join(", ")));
                docs.push_str(&format!("Version: {}\n", entry.package.version));
                if let Some(updated_at) = &entry.updated_at
                    && let Ok(formatted) = updated_at.format(&yyyy_mm_dd())
                {
                    docs.push_str(&format!("Updated: {formatted}\n"));
                }
                (format_str!("{}", entry.spec()), docs)
            })
            .collect_vec(); // avoid self borrow
        for (value, docs) in completion_info {
            self.value_completion(None, &Value::Str(value), false, Some(&docs));
        }
    }

    /// Add completions for raw block tags.
    pub fn raw_completions(&mut self) {
        for (name, mut tags) in RawElem::languages() {
            let lower = name.to_lowercase();
            if !tags.contains(&lower.as_str()) {
                tags.push(lower.as_str());
            }

            tags.retain(|tag| is_ident(tag));
            if tags.is_empty() {
                continue;
            }

            self.push_completion(Completion {
                kind: CompletionKind::Constant,
                label: name.into(),
                apply: Some(tags[0].into()),
                detail: Some(repr::separated_list(&tags, " or ").into()),
                ..Completion::default()
            });
        }
    }

    /// Add completions for labels and references.
    pub fn ref_completions(&mut self) {
        self.label_completions_(false, true);
    }

    /// Add completions for labels and references.
    pub fn label_completions(&mut self, only_citation: bool) {
        self.label_completions_(only_citation, false);
    }

    fn label_completion_apply(
        &mut self,
        label: &EcoString,
        open: bool,
        close: bool,
    ) -> (EcoString, Option<Vec<EcoTextEdit>>) {
        if is_valid_label_literal_id(label.as_str()) {
            return (
                eco_format!(
                    "{}{}{}",
                    if open { "<" } else { "" },
                    label.as_str(),
                    if close { ">" } else { "" }
                ),
                None,
            );
        }
        let mut edits = vec![];

        let (remove_open, remove_close) = match self.cursor.selected_node() {
            Some(SelectedNode::Label(node)) => {
                let range = node.range();
                let remove_open = node
                    .leaf_text()
                    .starts_with('<')
                    .then_some(range.start..range.start + 1);
                let remove_close = node
                    .leaf_text()
                    .ends_with('>')
                    .then_some(range.end - 1..range.end);
                (remove_open, remove_close)
            }
            _ => (
                (self.cursor.from > 0 && self.cursor.text[..self.cursor.from].ends_with('<'))
                    .then_some(self.cursor.from - 1..self.cursor.from),
                self.cursor
                    .after
                    .starts_with('>')
                    .then_some(self.cursor.cursor..self.cursor.cursor + 1),
            ),
        };

        if let Some(range) = remove_open {
            edits.push(EcoTextEdit::new(
                self.cursor.lsp_range_of(range),
                EcoString::new(),
            ));
        }
        if let Some(range) = remove_close {
            edits.push(EcoTextEdit::new(
                self.cursor.lsp_range_of(range),
                EcoString::new(),
            ));
        }

        (
            eco_format!("label({})", label.as_str().repr()),
            (!edits.is_empty()).then_some(edits),
        )
    }

    /// Add completions for label literals written in the source: the ones
    /// naming something (`name: <a>`) first, then by how close their enclosing
    /// syntax is to the cursor.
    ///
    /// Unlike [`Self::label_completions`], this doesn't need a compiled
    /// document, so it also works for labels that never land in the document,
    /// like node names passed to drawing packages (`node((0, 0), [A], name:
    /// <a>)`, then `edge(<a>, <b>)`).
    pub fn scoped_label_completions(&mut self, node: &LinkedNode) {
        let head = &self.cursor.text[..self.cursor.from];
        let open = !head.ends_with('<');
        let close = !self.cursor.after.starts_with('>');

        // Walks up the scopes (arguments, blocks, markup) around the cursor and
        // collects the labels of each one, the nearest scopes coming first.
        let mut found = vec![];
        let mut seen = HashMap::new();
        let mut depth = 0;
        let mut scope = node.parent().cloned();
        while let Some(current) = scope {
            if matches!(
                current.kind(),
                SyntaxKind::Args
                    | SyntaxKind::Array
                    | SyntaxKind::Dict
                    | SyntaxKind::Code
                    | SyntaxKind::CodeBlock
                    | SyntaxKind::Markup
                    | SyntaxKind::ContentBlock
            ) {
                collect_label_literals(
                    &current,
                    &self.worker.ctx.analysis.label_definition_params,
                    node.offset(),
                    depth,
                    &mut seen,
                    &mut found,
                );
                depth += 1;
            }
            scope = current.parent().cloned();
        }

        // Labels named through a variable (`let a = <a>`, then `name: a`) are
        // named as well.
        let params = &self.worker.ctx.analysis.label_definition_params;
        let named_by_variable: HashSet<_> = label_sites(&self.cursor.source, params)
            .into_iter()
            .filter(|site| site.kind == LabelSiteKind::Named)
            .map(|site| site.name)
            .collect();
        for lit in &mut found {
            lit.named |= named_by_variable.contains(&lit.name);
        }

        // Labels naming something (`name: <a>`) come first, as the others mostly
        // refer to them.
        found.sort_by_key(|lit| (!lit.named, lit.depth));

        // Keeps what the document knows about the labels that land in it.
        let in_document: HashMap<EcoString, DynLabel> = self
            .worker
            .document
            .map(|document| analyze_labels(document).0)
            .unwrap_or_default()
            .into_iter()
            .map(|label| (label.label.resolve().as_str().into(), label))
            .collect();

        for (rank, lit) in found.into_iter().enumerate() {
            let in_document = in_document.get(&lit.name);
            let (apply, additional_text_edits) =
                self.label_completion_apply(&lit.name, open, close);
            self.push_completion(Completion {
                kind: CompletionKind::Reference,
                apply: Some(apply),
                additional_text_edits,
                label: lit.name.clone(),
                label_details: in_document.and_then(|label| label.label_desc.clone()),
                filter_text: Some(lit.name),
                detail: in_document
                    .and_then(|label| label.detail.clone())
                    .or_else(|| lit.named.then(|| "name".into())),
                sort_text: Some(eco_format!("{rank:03}")),
                ..Completion::default()
            });
        }
    }

    /// Add completions for labels and references.
    pub fn label_completions_(&mut self, only_citation: bool, ref_label: bool) {
        self.worker.typed_label_completion |= only_citation || ref_label;
        let Some(document) = self.worker.document else {
            return;
        };
        let (labels, split) = analyze_labels(document);

        let head = &self.cursor.text[..self.cursor.from];
        let at = head.ends_with('@');
        let open = !at && !head.ends_with('<');
        let close = !at && !self.cursor.after.starts_with('>');
        let citation = !at && only_citation;

        let (skip, take) = if at || ref_label {
            (0, usize::MAX)
        } else if citation {
            (split, usize::MAX)
        } else {
            (0, split)
        };

        for DynLabel {
            label,
            label_desc,
            detail,
            bib_title,
        } in labels.into_iter().skip(skip).take(take)
        {
            if !self.worker.seen_casts.insert(hash128(&label)) {
                continue;
            }
            let label: EcoString = label.resolve().as_str().into();
            let (apply, additional_text_edits) = self.label_completion_apply(&label, open, close);
            let completion = Completion {
                kind: CompletionKind::Reference,
                apply: Some(apply),
                additional_text_edits,
                label: label.clone(),
                label_details: label_desc.clone(),
                filter_text: Some(label.clone()),
                detail: detail.clone(),
                ..Completion::default()
            };

            if let Some(bib_title) = bib_title {
                // Note that this completion re-uses the above `apply` field to
                // alter the `bib_title` to the corresponding label.
                self.push_completion(Completion {
                    kind: CompletionKind::Constant,
                    label: bib_title.clone(),
                    label_details: Some(label),
                    filter_text: Some(bib_title),
                    detail,
                    ..completion.clone()
                });
            }

            self.push_completion(completion);
        }
    }

    /// Add a completion for a specific value.
    pub fn value_completion(
        &mut self,
        label: Option<EcoString>,
        value: &Value,
        parens: bool,
        docs: Option<&str>,
    ) {
        self.value_completion_(
            value,
            ValueCompletionInfo {
                label,
                parens,
                label_details: None,
                docs,
                bound_self: false,
            },
        );
    }

    /// Add a completion for a specific value.
    pub fn value_completion_(&mut self, value: &Value, extras: ValueCompletionInfo) {
        let ValueCompletionInfo {
            label,
            parens,
            label_details,
            docs,
            bound_self,
        } = extras;

        // Prevent duplicate completions from appearing.
        if !self.worker.seen_casts.insert(hash128(&(&label, &value))) {
            return;
        }

        let at = label.as_deref().is_some_and(|field| !is_ident(field));
        let label = label.unwrap_or_else(|| value.repr());

        let detail = docs.map(Into::into).or_else(|| match value {
            Value::Symbol(symbol) => Some(symbol_detail(symbol.get())),
            Value::Func(func) => func.docs().map(plain_docs_sentence),
            Value::Type(ty) => Some(plain_docs_sentence(ty.docs())),
            v => {
                let repr = v.repr();
                (repr.as_str() != label).then_some(repr)
            }
        });
        let label_details = label_details.or_else(|| match value {
            Value::Symbol(s) => Some(symbol_label_detail(s.get())),
            _ => None,
        });

        let mut apply = None;
        if parens && matches!(value, Value::Func(_)) {
            let mode = self.cursor.leaf_mode();
            let ty = Ty::Value(InsTy::new(value.clone()));
            let kind_checker = CompletionKindChecker {
                symbols: HashSet::default(),
                functions: HashSet::from_iter([&ty]),
            };
            let mut fn_feat = FnCompletionFeat::default();
            // todo: unify bound self checking
            fn_feat.bound_self = bound_self;
            let fn_feat = fn_feat.check(kind_checker.functions.iter().copied());
            self.func_completion(mode, fn_feat, label, label_details, detail, parens);
            return;
        } else if at {
            apply = Some(eco_format!("at(\"{label}\")"));
        } else {
            let apply_label = &mut label.as_str();
            if apply_label.ends_with('"')
                && self.cursor.after.starts_with('"')
                && let Some(trimmed) = apply_label.strip_suffix('"')
            {
                *apply_label = trimmed;
            }
            let from_before = slice_at(self.cursor.text, 0..self.cursor.from);
            if apply_label.starts_with('"')
                && from_before.ends_with('"')
                && let Some(trimmed) = apply_label.strip_prefix('"')
            {
                *apply_label = trimmed;
            }

            if apply_label.len() != label.len() {
                apply = Some((*apply_label).into());
            }
        }

        self.push_completion(Completion {
            kind: value_to_completion_kind(value),
            label,
            apply,
            detail,
            label_details,
            ..Completion::default()
        });
    }

    pub fn symbol_completions(&mut self, label: EcoString, symbol: &Symbol) {
        let sym_val = symbol.get();
        let kind = CompletionKind::Symbol(sym_val.into());
        self.push_completion(Completion {
            kind,
            label: label.clone(),
            label_details: Some(symbol_label_detail(sym_val)),
            detail: Some(symbol_detail(sym_val)),
            ..Completion::default()
        });

        let is_stepless = self.cursor.ctx.analysis.completion_feat.is_stepless();
        if is_stepless {
            self.symbol_var_completions(symbol, Some(&label));
        }
    }

    pub fn symbol_var_completions(&mut self, symbol: &Symbol, prefix: Option<&str>) {
        for modifier in symbol.modifiers() {
            if let Ok(modified) = symbol.clone().modified((), modifier) {
                let label = match &prefix {
                    Some(prefix) => eco_format!("{prefix}.{modifier}"),
                    None => modifier.into(),
                };

                self.symbol_completions(label, &modified);
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ValueCompletionInfo<'a> {
    pub label: Option<EcoString>,
    pub parens: bool,
    pub label_details: Option<EcoString>,
    pub docs: Option<&'a str>,
    pub bound_self: bool,
}

/// A label literal found in the source by [`collect_label_literals`].
struct LabelLiteral {
    /// The label, without the angle brackets.
    name: EcoString,
    /// How many scopes away from the cursor it was found.
    depth: usize,
    /// Whether it defines the label, like `name: <label>`.
    named: bool,
}

/// Collects the label literals under `node`, skipping the one at `cursor_node`.
/// A label already `seen` keeps its nearest scope, and is marked as named if any
/// of its occurrences is.
fn collect_label_literals(
    node: &LinkedNode,
    params: &LabelDefinitionParams,
    cursor_node: usize,
    depth: usize,
    seen: &mut HashMap<EcoString, usize>,
    found: &mut Vec<LabelLiteral>,
) {
    for child in node.children() {
        if child.kind() == SyntaxKind::Label {
            if child.offset() == cursor_node {
                continue;
            }
            let text = child.leaf_text();
            let name: EcoString = text.trim_start_matches('<').trim_end_matches('>').into();
            if name.is_empty() {
                continue;
            }
            let named = label_site_kind(&child, params) == LabelSiteKind::Named;
            match seen.get(&name) {
                Some(&idx) => found[idx].named |= named,
                None => {
                    seen.insert(name.clone(), found.len());
                    found.push(LabelLiteral { name, depth, named });
                }
            }
        } else {
            collect_label_literals(&child, params, cursor_node, depth, seen, found);
        }
    }
}
