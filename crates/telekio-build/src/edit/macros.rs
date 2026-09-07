use std::error::Error;

use ra_ap_syntax::{
    AstNode, SyntaxKind, SyntaxNode,
    ast::{self, make},
};

use super::{
    Scope, commit, count_targets, count_token_tree, open, parse, replace_token_tree,
    retain_outermost, token_tree_source,
};

pub fn retarget_macro(
    source: &mut String,
    scope: Scope<'_>,
    from: &str,
    to: &str,
) -> Result<(), Box<dyn Error>> {
    match count_targets(source, scope, &scope_matches, &nested_scope)? {
        0 => return Err("macro target scope was not found".into()),
        1 => {}
        _ => return Err("macro target scope appears more than once".into()),
    }
    let count = count_targets(
        source,
        (scope, from),
        &|root, (scope, from)| Ok(macro_paths(root, scope, from)?.len()),
        &|(scope, from), tree, inner| scope.inside(tree).map(|scope| ((scope, from), inner)),
    )?;
    match count {
        0 => return Err(format!("scope is not enclosed by macro `{from}`").into()),
        1 => {}
        _ => return Err(format!("more than one macro `{from}` encloses the scope").into()),
    }
    if retarget_macro_in(source, scope, from, to)? {
        Ok(())
    } else {
        Err(format!("scope is not enclosed by macro `{from}`").into())
    }
}

fn scope_matches(root: &SyntaxNode, scope: Scope<'_>) -> Result<usize, Box<dyn Error>> {
    Ok(usize::from(scope.resolve(root)?.is_some()))
}

fn nested_scope<'a>(
    scope: Scope<'a>,
    tree: &ast::TokenTree,
    inner: String,
) -> Option<(Scope<'a>, String)> {
    scope.inside(tree).map(|scope| (scope, inner))
}

fn macro_paths(
    root: &SyntaxNode,
    scope: Scope<'_>,
    name: &str,
) -> Result<Vec<ast::Path>, Box<dyn Error>> {
    let mut paths = Vec::new();
    for call in root.descendants().filter_map(ast::MacroCall::cast) {
        let (Some(path), Some(tree)) = (call.path(), call.token_tree()) else {
            continue;
        };
        let spelling = path
            .syntax()
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| !matches!(token.kind(), SyntaxKind::WHITESPACE | SyntaxKind::COMMENT))
            .map(|token| token.text().to_owned())
            .collect::<String>();
        if spelling == name && count_token_tree(&tree, scope, &scope_matches, &nested_scope)? != 0 {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn retarget_macro_in(
    source: &mut String,
    scope: Scope<'_>,
    from: &str,
    to: &str,
) -> Result<bool, Box<dyn Error>> {
    let (editor, root) = open(source)?;
    let paths = macro_paths(&root, scope, from)?;
    if let [path] = paths.as_slice() {
        editor.replace(path.syntax(), make::path_from_text(to).syntax().clone());
        commit(source, editor)?;
        return Ok(true);
    }

    let mut replacements = Vec::new();
    for tree in root.descendants().filter_map(ast::TokenTree::cast) {
        let Some(inner_scope) = scope.inside(&tree) else {
            continue;
        };
        let Some(mut inner) = token_tree_source(&tree) else {
            continue;
        };
        if parse(&inner).is_err() || !retarget_macro_in(&mut inner, inner_scope, from, to)? {
            continue;
        }
        let replacement = replace_token_tree(&tree, &inner)?;
        retain_outermost(&mut replacements, tree, replacement);
    }
    if replacements.is_empty() {
        return Ok(false);
    }
    for (old, new) in replacements {
        editor.replace(old.syntax(), new.syntax().clone());
    }
    commit(source, editor)?;
    Ok(true)
}
