//! Label literals written in the source, sorted into definitions and uses.
//!
//! Besides the labels attached to content, some packages take labels as names
//! for the things they draw (`node((0, 0), [A], name: <a>)`, then `edge(<a>,
//! <b>)`). Such labels never land in the document, so they are tracked
//! syntactically here, like variables: `name: <a>` defines `<a>`, and the other
//! label literals use it. Which parameters define labels is configurable, see
//! [`LabelDefinitionParams`].

use std::collections::HashSet;

use tinymist_analysis::analyze_labels;
use tinymist_analysis::syntax::{InterpretMode, find_source_by_expr, interpret_mode_at};
use tinymist_project::LspWorld;
use tinymist_world::vfs::WorkspaceResolver;
use typst::diag::{SourceDiagnostic, eco_format};
use typst::syntax::{Span, VirtualRoot};

use super::SharedContext;
use crate::prelude::*;

/// The names of the parameters whose label arguments define a label, like
/// `name` in `node((0, 0), [A], name: <a>)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LabelDefinitionParams(pub EcoVec<EcoString>);

impl Default for LabelDefinitionParams {
    fn default() -> Self {
        Self(eco_vec!["name".into()])
    }
}

impl LabelDefinitionParams {
    /// Whether a label passed to the parameter `param` defines the label.
    pub fn contains(&self, param: &str) -> bool {
        self.0.iter().any(|name| name == param)
    }
}

/// How a label literal is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSiteKind {
    /// Passed to a definition parameter like `name: <a>`, defining a named
    /// thing.
    Named,
    /// Attached to content in markup, like `= Heading <a>`.
    Attached,
    /// Bound to a variable, like `let a = <a>`. The variable then stands for
    /// the label: `name: a` defines it, and other uses of `a` use it.
    Bound,
    /// Any other label literal, using a label defined elsewhere.
    Use,
}

impl LabelSiteKind {
    /// Whether the label literal defines the label.
    pub fn is_definition(self) -> bool {
        matches!(self, Self::Named | Self::Attached)
    }
}

/// A label written in the source, as a literal or as a variable bound to one.
#[derive(Debug, Clone)]
pub struct LabelSite {
    /// The label, without the angle brackets.
    pub name: EcoString,
    /// The span of the label literal, or of the variable standing for it.
    pub span: Span,
    /// How the label literal is written.
    pub kind: LabelSiteKind,
}

/// Classifies a label literal, or a variable bound to a label.
pub fn label_site_kind(node: &LinkedNode, params: &LabelDefinitionParams) -> LabelSiteKind {
    let named = node
        .parent()
        .and_then(|named| named.cast::<ast::Named>())
        .is_some_and(|named| {
            params.contains(named.name().as_str()) && named.expr().span() == node.span()
        });
    if named {
        LabelSiteKind::Named
    } else if node.parent_kind() == Some(SyntaxKind::LetBinding) {
        LabelSiteKind::Bound
    } else if matches!(interpret_mode_at(Some(node)), InterpretMode::Markup) {
        LabelSiteKind::Attached
    } else {
        LabelSiteKind::Use
    }
}

/// Collects the labels of a source, in source order: the label literals, and
/// the variables bound to a label literal (`let a = <a>`) where they stand for
/// it.
pub fn label_sites(source: &Source, params: &LabelDefinitionParams) -> Vec<LabelSite> {
    let root = LinkedNode::new(source.root());
    let mut bound = HashMap::new();
    collect_bound(&root, &mut bound);
    let mut sites = vec![];
    collect(&root, params, &bound, &mut sites);
    sites
}

/// Collects the variables bound to a label literal, like `let a = <a>`.
fn collect_bound(node: &LinkedNode, bound: &mut HashMap<EcoString, EcoString>) {
    if let Some(binding) = node.cast::<ast::LetBinding>()
        && let ast::LetBindingKind::Normal(ast::Pattern::Normal(ast::Expr::Ident(ident))) =
            binding.kind()
        && let Some(ast::Expr::Label(label)) = binding.init()
    {
        bound.insert(ident.get().clone(), label.get().into());
    }
    for child in node.children() {
        collect_bound(&child, bound);
    }
}

fn collect(
    node: &LinkedNode,
    params: &LabelDefinitionParams,
    bound: &HashMap<EcoString, EcoString>,
    sites: &mut Vec<LabelSite>,
) {
    for child in node.children() {
        let name = match child.kind() {
            SyntaxKind::Label => {
                let name = child.leaf_text();
                Some(name.trim_start_matches('<').trim_end_matches('>').into())
            }
            // A variable standing for a label, except where it is bound, or names
            // an argument or a field.
            SyntaxKind::Ident if is_variable_use(&child) => bound.get(child.leaf_text()).cloned(),
            _ => None,
        };
        match name {
            Some(name) if !name.is_empty() => sites.push(LabelSite {
                name,
                span: child.span(),
                kind: label_site_kind(&child, params),
            }),
            Some(_) => {}
            None => collect(&child, params, bound, sites),
        }
    }
}

/// Whether an identifier reads a variable, rather than binding it or naming an
/// argument (`name: …`) or a field (`a.name`).
fn is_variable_use(ident: &LinkedNode) -> bool {
    let Some(parent) = ident.parent() else {
        return true;
    };
    match parent.kind() {
        SyntaxKind::LetBinding => false,
        SyntaxKind::Named => parent
            .cast::<ast::Named>()
            .is_some_and(|named| named.expr().span() == ident.span()),
        SyntaxKind::FieldAccess => parent
            .cast::<ast::FieldAccess>()
            .is_some_and(|access| access.target().span() == ident.span()),
        _ => true,
    }
}

/// Finds where a label is defined: in `source` first, then in the files it
/// imports, then in the other files the document depends on.
pub fn find_label_definition(
    ctx: &Arc<SharedContext>,
    source: &Source,
    name: &str,
) -> Option<LabelSite> {
    let in_file = |source: &Source| {
        let sites = label_sites(source, &ctx.analysis.label_definition_params);
        let mut defs = sites
            .into_iter()
            .filter(|site| site.name == name && site.kind.is_definition());
        let first = defs.next()?;
        // `name: <a>` is preferred over a label attached to content.
        Some(match first.kind {
            LabelSiteKind::Named => first,
            _ => defs
                .find(|site| site.kind == LabelSiteKind::Named)
                .unwrap_or(first),
        })
    };

    related_sources(ctx, source).iter().find_map(in_file)
}

/// The sources a label of `source` may be defined in: `source` itself, the
/// files it imports or includes (transitively, packages included), then the
/// other files the document depends on.
///
/// Imports are followed syntactically, so this also works while the document
/// doesn't compile.
pub fn related_sources(ctx: &SharedContext, source: &Source) -> Vec<Source> {
    let world = ctx.world();
    let mut seen = HashSet::from([source.id()]);
    let mut sources = vec![source.clone()];
    let mut next = 0;
    while let Some(current) = sources.get(next).cloned() {
        next += 1;
        let mut imported = vec![];
        collect_imported(
            world,
            &LinkedNode::new(current.root()),
            current.id(),
            &mut imported,
        );
        for source in imported {
            if seen.insert(source.id()) {
                sources.push(source);
            }
        }
    }

    for fid in world.depended_files() {
        if is_typst_file(fid)
            && seen.insert(fid)
            && let Ok(source) = ctx.source_by_id(fid)
        {
            sources.push(source);
        }
    }
    sources
}

fn collect_imported(
    world: &LspWorld,
    node: &LinkedNode,
    current: TypstFileId,
    out: &mut Vec<Source>,
) {
    let path = match node.cast::<ast::Expr>() {
        Some(ast::Expr::ModuleImport(import)) => Some(import.source()),
        Some(ast::Expr::ModuleInclude(include)) => Some(include.source()),
        _ => None,
    };
    if let Some(source) = path.and_then(|path| find_source_by_expr(world, current, path)) {
        out.push(source);
    }
    for child in node.children() {
        collect_imported(world, &child, current, out);
    }
}

/// Whether the file is a Typst source file.
pub(crate) fn is_typst_file(fid: TypstFileId) -> bool {
    fid.vpath()
        .as_rooted_path_compat()
        .extension()
        .is_some_and(|ext| ext == "typ")
}

/// Describes where a label is defined, for hovering one of its uses: the file,
/// the line, and the call it names (`cas((1, 0), [A], name: <a>)`) or the line
/// it is attached on (`= Heading <a>`).
pub fn describe_label_definition(
    ctx: &SharedContext,
    from: TypstFileId,
    def: &LabelSite,
) -> Option<String> {
    let fid = def.span.id()?;
    let source = ctx.source_by_id(fid).ok()?;
    let node = LinkedNode::new(source.root()).find(def.span)?;
    let line = source.lines().byte_to_line(node.offset())?;

    let snippet: EcoString = match def.kind {
        LabelSiteKind::Named => node
            .parent()
            .and_then(|named| named.parent())
            .and_then(|args| args.parent())
            .filter(|call| call.kind() == SyntaxKind::FuncCall)
            .map(|call| source.text()[call.range()].into())
            .unwrap_or_else(|| source.text()[node.range()].into()),
        _ => source
            .lines()
            .line_to_range(line)
            .and_then(|range| source.text().get(range))
            .unwrap_or_default()
            .trim()
            .into(),
    };
    let snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ");

    let place = if fid == from {
        format!("Defined on line {}", line + 1)
    } else {
        let file = match fid.root() {
            VirtualRoot::Package(spec) if WorkspaceResolver::is_package_file(fid) => {
                format!("{spec}/{}", fid.vpath().get_without_slash())
            }
            _ => fid.vpath().get_without_slash().to_owned(),
        };
        format!("Defined in `{file}`, line {}", line + 1)
    };
    let lang = if def.kind == LabelSiteKind::Named {
        "typc"
    } else {
        "typ"
    };
    Some(format!("{place}:\n```{lang}\n{snippet}\n```"))
}

/// Checks the labels of a source like variables: a label used but defined
/// nowhere, or a named thing (`name: <a>`) that is never used.
///
/// The definitions and uses are looked up in all the files the document depends
/// on, and the labels of the compiled document count as defined.
pub fn label_diagnostics(ctx: &SharedContext, source: &Source) -> EcoVec<SourceDiagnostic> {
    let mut defined = HashSet::new();
    let mut used = HashSet::new();
    let mut here = vec![];

    for (idx, file) in related_sources(ctx, source).into_iter().enumerate() {
        let is_here = idx == 0;
        for site in label_sites(&file, &ctx.analysis.label_definition_params) {
            match site.kind {
                LabelSiteKind::Named | LabelSiteKind::Attached => {
                    defined.insert(site.name.clone());
                }
                LabelSiteKind::Use => {
                    used.insert(site.name.clone());
                }
                LabelSiteKind::Bound => {}
            }
            if is_here {
                here.push(site);
            }
        }
    }
    if let Some(document) = ctx.success_doc() {
        let (labels, _) = analyze_labels(document);
        defined.extend(
            labels
                .into_iter()
                .map(|l| l.label.resolve().as_str().into()),
        );
    }

    let mut diagnostics = EcoVec::new();
    for site in here {
        match site.kind {
            LabelSiteKind::Use if !defined.contains(&site.name) => {
                diagnostics.push(
                    SourceDiagnostic::warning(
                        site.span,
                        eco_format!("label `<{}>` is not defined", site.name),
                    )
                    .with_hint(eco_format!(
                        "name something with `name: <{}>`, or attach the label to content",
                        site.name
                    )),
                );
            }
            LabelSiteKind::Named if !used.contains(&site.name) => {
                diagnostics.push(SourceDiagnostic::warning(
                    site.span,
                    eco_format!("label `<{}>` is never used", site.name),
                ));
            }
            _ => {}
        }
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str, params: &[&str]) -> Vec<(EcoString, LabelSiteKind)> {
        let params = LabelDefinitionParams(params.iter().map(|&p| p.into()).collect());
        label_sites(&Source::detached(text), &params)
            .into_iter()
            .map(|site| (site.name, site.kind))
            .collect()
    }

    #[test]
    fn label_definition_params() {
        use LabelSiteKind::*;
        let text = "= Title <title>\n#f(name: <a>, id: <b>, target: <a>)\n#f(<b>)";

        let default = LabelDefinitionParams::default();
        assert_eq!(default.0.as_slice(), ["name"]);
        assert_eq!(
            kinds(text, &["name"]),
            [
                ("title".into(), Attached),
                ("a".into(), Named),
                ("b".into(), Use),
                ("a".into(), Use),
                ("b".into(), Use),
            ]
        );
        assert_eq!(
            kinds(text, &["name", "id"]),
            [
                ("title".into(), Attached),
                ("a".into(), Named),
                ("b".into(), Named),
                ("a".into(), Use),
                ("b".into(), Use),
            ]
        );
    }

    #[test]
    fn label_bound_to_variable() {
        use LabelSiteKind::*;
        let text = "#let a = <a>\n#f(name: a, other: a, a)\n#let name = 1\n#f(x.a)";
        assert_eq!(
            kinds(text, &["name"]),
            [
                ("a".into(), Bound),
                ("a".into(), Named),
                ("a".into(), Use),
                ("a".into(), Use),
            ]
        );
    }
}
