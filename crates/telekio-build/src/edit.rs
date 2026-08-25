use std::{error::Error, path::Path};

use ra_ap_syntax::{
    AstNode, Edition, SourceFile, SyntaxElement, SyntaxKind, SyntaxNode,
    ast::{self, HasArgList, HasGenericParams, HasName, edit::IndentLevel, make},
    syntax_editor::{Position, SyntaxEditor},
};

pub struct Field<'a> {
    pub visibility: Option<&'a str>,
    pub name: &'a str,
    pub ty: &'a str,
}

pub struct FieldInit<'a> {
    pub name: &'a str,
    pub value: &'a str,
}

#[derive(Clone, Copy)]
pub enum AttrTarget<'a> {
    Struct(&'a str),
    Enum(&'a str),
    Module(&'a str),
    Method { owner: &'a str, name: &'a str },
    Impl { owner: &'a str, method: &'a str },
}

pub fn add_attr(
    source: &mut String,
    target: AttrTarget<'_>,
    attribute: &str,
) -> Result<(), Box<dyn Error>> {
    if add_attr_in(source, target, attribute)? {
        Ok(())
    } else {
        Err("attribute target was not found".into())
    }
}

fn add_attr_in(
    source: &mut String,
    target: AttrTarget<'_>,
    attribute: &str,
) -> Result<bool, Box<dyn Error>> {
    let (editor, root) = open(source)?;
    let items = attr_targets(&root, target)?;
    if !items.is_empty() {
        let attribute = parse(&format!("{attribute}\nfn replacement() {{}}"))?
            .syntax()
            .descendants()
            .find_map(ast::Attr::cast)
            .ok_or("wrapper has no attribute")?;
        for item in items {
            let indent = IndentLevel::from_node(&item);
            editor.insert_all(
                Position::first_child_of(&item),
                vec![
                    attribute.syntax().clone().into(),
                    make::tokens::whitespace(&format!("\n{indent}")).into(),
                ],
            );
        }
        commit(source, editor)?;
        return Ok(true);
    }

    let mut replacements = Vec::new();
    for source_tree in root.descendants().filter_map(ast::TokenTree::cast) {
        let text = source_tree.syntax().text().to_string();
        let Some(inner) = text
            .strip_prefix('{')
            .and_then(|text| text.strip_suffix('}'))
            .map(str::to_owned)
        else {
            continue;
        };
        let owner = source_tree
            .syntax()
            .ancestors()
            .find_map(ast::Impl::cast)
            .and_then(|implementation| implementation.self_ty())
            .map(|ty| ty.syntax().text().to_string());
        let mut edited = owner.as_ref().map_or_else(
            || inner.clone(),
            |owner| format!("impl {owner} {{ {inner} }}"),
        );
        if parse(&edited).is_err() || !add_attr_in(&mut edited, target, attribute)? {
            continue;
        }
        let inner = if let Some(owner) = owner {
            let file = parse(&edited)?;
            let implementation = one(
                file.syntax()
                    .descendants()
                    .filter_map(ast::Impl::cast)
                    .filter(|implementation| {
                        implementation
                            .self_ty()
                            .is_some_and(|ty| ty.syntax().text() == owner.as_str())
                    }),
                &format!("wrapper impl `{owner}`"),
            )?;
            let list = implementation
                .assoc_item_list()
                .ok_or("wrapper impl has no item list")?;
            let text = list.syntax().text().to_string();
            text.strip_prefix('{')
                .and_then(|text| text.strip_suffix('}'))
                .ok_or("wrapper impl does not use braces")?
                .to_owned()
        } else {
            edited
        };
        let replacement_tree = parse(&format!("replacement! {{ {inner} }}"))?
            .syntax()
            .descendants()
            .find_map(ast::MacroCall::cast)
            .and_then(|call| call.token_tree())
            .ok_or("replacement macro has no token tree")?;
        let range = source_tree.syntax().text_range();
        if replacements
            .iter()
            .any(|(existing, _): &(ast::TokenTree, ast::TokenTree)| {
                let existing = existing.syntax().text_range();
                existing.start() <= range.start() && existing.end() >= range.end()
            })
        {
            continue;
        }
        replacements.retain(|(existing, _)| {
            let existing = existing.syntax().text_range();
            !(range.start() <= existing.start() && range.end() >= existing.end())
        });
        replacements.push((source_tree, replacement_tree));
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

pub fn set_type_parameter(
    source: &mut String,
    owner: &str,
    method: &str,
    parameter: &str,
    declaration: &str,
) -> Result<(), Box<dyn Error>> {
    let (editor, root) = open(source)?;
    let implementation = one(
        root.descendants()
            .filter_map(ast::Impl::cast)
            .filter(|implementation| {
                implementation
                    .self_ty()
                    .is_some_and(|ty| ty.syntax().text() == owner)
                    && implementation
                        .assoc_item_list()
                        .into_iter()
                        .flat_map(|items| items.assoc_items())
                        .any(|item| {
                            matches!(item, ast::AssocItem::Fn(function) if function.name().is_some_and(|name| name.text() == method))
                        })
            }),
        &format!("impl `{owner}` containing `{method}`"),
    )?;
    let parameter = one(
        implementation
            .generic_param_list()
            .into_iter()
            .flat_map(|parameters| parameters.generic_params())
            .filter_map(|parameter| match parameter {
                ast::GenericParam::TypeParam(parameter) => Some(parameter),
                _ => None,
            })
            .filter(|candidate| {
                candidate
                    .name()
                    .is_some_and(|name| name.text() == parameter)
            }),
        &format!("type parameter `{parameter}` in impl `{owner}`"),
    )?;
    let replacement = parse(&format!("fn replacement<{declaration}>() {{}}"))?
        .syntax()
        .descendants()
        .find_map(ast::TypeParam::cast)
        .ok_or("replacement has no type parameter")?;
    editor.replace(parameter.syntax(), replacement.syntax().clone());
    commit(source, editor)
}

pub fn rename_method(
    source: &mut String,
    owner: &str,
    name: &str,
    replacement: &str,
) -> Result<(), Box<dyn Error>> {
    let (editor, root) = open(source)?;
    let method = method(&root, owner, name)?;
    let name = method.name().ok_or("method has no name")?;
    editor.replace(name.syntax(), make::name(replacement).syntax().clone());
    commit(source, editor)
}

pub fn append_fields(
    source: &mut String,
    name: &str,
    fields: &[Field<'_>],
) -> Result<(), Box<dyn Error>> {
    let (editor, root) = open(source)?;
    let item = named::<ast::Struct>(&root, name)?;
    let Some(ast::FieldList::RecordFieldList(list)) = item.field_list() else {
        return Err(format!("struct `{name}` has no record fields").into());
    };
    let close = list.r_curly_token().ok_or("struct has no closing brace")?;
    let indent = IndentLevel::from_node(list.syntax()) + 1;
    let mut elements = Vec::new();
    for field in fields {
        elements.extend([
            make::tokens::whitespace(&indent.to_string()).into(),
            make::record_field(
                field.visibility.map(visibility).transpose()?,
                make::name(field.name),
                make::ty(field.ty),
            )
            .syntax()
            .clone()
            .into(),
            make::token(ra_ap_syntax::SyntaxKind::COMMA).into(),
            make::tokens::whitespace("\n").into(),
        ]);
    }
    editor.insert_all(Position::before(close), elements);
    commit(source, editor)
}

pub fn append_record_fields(
    source: &mut String,
    scope: Scope<'_>,
    record: &str,
    fields: &[FieldInit<'_>],
) -> Result<(), Box<dyn Error>> {
    let (editor, root) = open(source)?;
    let scope = scope
        .resolve(&root)?
        .ok_or("record initializer scope was not found")?;
    let record = one(
        scope
            .descendants()
            .filter_map(ast::RecordExpr::cast)
            .filter(|expression| {
                expression.path().is_some_and(|path| {
                    path.segment()
                        .and_then(|segment| segment.name_ref())
                        .is_some_and(|candidate| candidate.text() == record)
                })
            }),
        &format!("`{record}` initializer"),
    )?;
    let list = record
        .record_expr_field_list()
        .ok_or("record initializer has no fields")?;
    list.add_fields(
        &editor,
        fields
            .iter()
            .map(|field| {
                Ok(make::record_expr_field(
                    make::name_ref(field.name),
                    Some(expression(field.value)?),
                ))
            })
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?,
    );
    commit(source, editor)
}

pub fn retarget_use(source: &mut String, name: &str, path: &str) -> Result<(), Box<dyn Error>> {
    if retarget_use_in(source, name, path)? {
        Ok(())
    } else {
        Err(format!("use tree `{name}` was not found").into())
    }
}

fn retarget_use_in(source: &mut String, name: &str, path: &str) -> Result<bool, Box<dyn Error>> {
    let (editor, root) = open(source)?;
    let matches = root
        .descendants()
        .filter_map(ast::UseTree::cast)
        .filter(|tree| {
            tree.path()
                .and_then(|path| path.segment())
                .and_then(|segment| segment.name_ref())
                .is_some_and(|candidate| candidate.text() == name)
                && tree.rename().is_none()
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [tree] if tree.use_tree_list().is_some() => {
            let old = tree.path().ok_or("use tree has no path")?;
            editor.replace(old.syntax(), make::path_from_text(path).syntax().clone());
            commit(source, editor)?;
            return Ok(true);
        }
        [tree]
            if tree
                .syntax()
                .ancestors()
                .find_map(ast::UseTreeList::cast)
                .is_some() =>
        {
            for element in use_tree_removal(tree) {
                editor.delete(element);
            }
            commit(source, editor)?;
            add_use(source, path)?;
            return Ok(true);
        }
        [tree] => {
            let old = tree.path().ok_or("use tree has no path")?;
            editor.replace(old.syntax(), make::path_from_text(path).syntax().clone());
            commit(source, editor)?;
            return Ok(true);
        }
        [] => {}
        _ => return Err(format!("found multiple use trees `{name}`").into()),
    }

    let mut replacements = Vec::new();
    for source_tree in root.descendants().filter_map(ast::TokenTree::cast) {
        let text = source_tree.syntax().text().to_string();
        let Some(mut inner) = text
            .strip_prefix('{')
            .and_then(|text| text.strip_suffix('}'))
            .map(str::to_owned)
        else {
            continue;
        };
        if parse(&inner).is_err() || !retarget_use_in(&mut inner, name, path)? {
            continue;
        }
        let replacement = parse(&format!("replacement! {{ {inner} }}"))?
            .syntax()
            .descendants()
            .find_map(ast::MacroCall::cast)
            .and_then(|call| call.token_tree())
            .ok_or("replacement macro has no token tree")?;
        replacements.push((source_tree, replacement));
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

pub fn delegate_closure(
    source: &mut String,
    scope: Scope<'_>,
    callee: &str,
    index: usize,
    helper: &str,
    context: &[&str],
) -> Result<(), Box<dyn Error>> {
    if delegate_closure_in(source, scope, callee, index, helper, context)? {
        Ok(())
    } else {
        Err(format!("scope for `{callee}` call was not found").into())
    }
}

fn delegate_closure_in(
    source: &mut String,
    scope: Scope<'_>,
    callee: &str,
    index: usize,
    helper: &str,
    context: &[&str],
) -> Result<bool, Box<dyn Error>> {
    let (editor, root) = open(source)?;
    if let Some(scope) = scope.resolve(&root)? {
        let calls = scope
            .descendants()
            .filter_map(ast::CallExpr::cast)
            .filter_map(|call| match call.expr() {
                Some(ast::Expr::PathExpr(path)) => path
                    .path()
                    .map(|path| (path.syntax().text().to_string(), call)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let calls = calls
            .iter()
            .filter(|(path, _)| path == callee)
            .map(|(_, call)| call.clone())
            .collect::<Vec<_>>();
        let call = match calls.as_slice() {
            [] => return Ok(false),
            [call] => call.clone(),
            _ => return Err(format!("more than one `{callee}` call in selected scope").into()),
        };
        let argument = call
            .arg_list()
            .and_then(|arguments| arguments.args().nth(index))
            .ok_or_else(|| format!("`{callee}` has no argument {index}"))?;
        if !matches!(argument, ast::Expr::ClosureExpr(_)) {
            return Err(format!("argument {index} to `{callee}` is not a closure").into());
        }
        let mut arguments = context
            .iter()
            .map(|source| expression(source))
            .collect::<Result<Vec<_>, _>>()?;
        arguments.push(argument.clone());
        let delegate = make::expr_call(expression(helper)?, make::arg_list(arguments));
        editor.replace(argument.syntax(), delegate.syntax().clone());
        commit(source, editor)?;
        return Ok(true);
    }

    let mut replacements = Vec::new();
    for source_tree in root.descendants().filter_map(ast::TokenTree::cast) {
        let text = source_tree.syntax().text().to_string();
        let Some(mut inner) = text
            .strip_prefix('{')
            .and_then(|text| text.strip_suffix('}'))
            .map(str::to_owned)
        else {
            continue;
        };
        if parse(&inner).is_err()
            || !delegate_closure_in(&mut inner, scope, callee, index, helper, context)?
        {
            continue;
        }
        let replacement = parse(&format!("replacement! {{ {inner} }}"))?
            .syntax()
            .descendants()
            .find_map(ast::MacroCall::cast)
            .and_then(|call| call.token_tree())
            .ok_or("replacement macro has no token tree")?;
        retain_outermost(&mut replacements, source_tree, replacement);
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

pub fn mount_module(
    source: &mut String,
    visibility: Option<&str>,
    name: &str,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    let visibility = visibility.map_or(String::new(), |visibility| format!("{visibility} "));
    let file = parse(&format!(
        "#[path = {:?}]\n{visibility}mod {name};",
        path.to_string_lossy()
    ))?;
    let module = one(
        file.syntax().children().filter_map(ast::Module::cast),
        "module in wrapper",
    )?;
    let (editor, root) = open(source)?;
    let anchor = root
        .children()
        .find(|node| ast::Item::can_cast(node.kind()))
        .ok_or("source has no items")?;
    editor.insert_all(
        Position::before(&anchor),
        vec![
            module.syntax().clone().into(),
            make::tokens::whitespace("\n\n").into(),
        ],
    );
    commit(source, editor)
}

#[derive(Clone, Copy)]
pub enum Scope<'a> {
    Function(&'a str),
    Method { owner: &'a str, name: &'a str },
}

pub fn redirect_call(
    source: &mut String,
    scope: Scope<'_>,
    from: &str,
    to: &str,
) -> Result<(), Box<dyn Error>> {
    if redirect_call_in(source, scope, from, to)? {
        Ok(())
    } else {
        Err(format!("scope for `{from}` call was not found").into())
    }
}

fn redirect_call_in(
    source: &mut String,
    scope: Scope<'_>,
    from: &str,
    to: &str,
) -> Result<bool, Box<dyn Error>> {
    let (editor, root) = open(source)?;
    if let Some(scope) = scope.resolve(&root)? {
        let methods = scope
            .descendants()
            .filter_map(ast::MethodCallExpr::cast)
            .filter(|call| call.name_ref().is_some_and(|name| name.text() == from))
            .filter_map(|call| call.name_ref().map(|name| (name.syntax().clone(), true)));
        let functions = scope
            .descendants()
            .filter_map(ast::CallExpr::cast)
            .filter_map(|call| match call.expr() {
                Some(ast::Expr::PathExpr(expression)) => expression.path(),
                _ => None,
            })
            .filter(|path| path.syntax().text() == from)
            .map(|path| (path.syntax().clone(), false));
        let calls = methods.chain(functions).collect::<Vec<_>>();
        let (callee, method) = match calls.as_slice() {
            [] => return Ok(false),
            [call] => call.clone(),
            _ => return Err(format!("more than one `{from}` call in selected scope").into()),
        };
        if method {
            if to.contains("::") {
                return Err(format!("method replacement `{to}` is not a name").into());
            }
            editor.replace(callee, make::name_ref(to).syntax().clone());
        } else {
            editor.replace(callee, make::path_from_text(to).syntax().clone());
        }
        commit(source, editor)?;
        return Ok(true);
    }

    let mut replacements = Vec::new();
    for source_tree in root.descendants().filter_map(ast::TokenTree::cast) {
        let text = source_tree.syntax().text().to_string();
        let Some(mut inner) = text
            .strip_prefix('{')
            .and_then(|text| text.strip_suffix('}'))
            .map(str::to_owned)
        else {
            continue;
        };
        if parse(&inner).is_err() || !redirect_call_in(&mut inner, scope, from, to)? {
            continue;
        }
        let replacement = parse(&format!("replacement! {{ {inner} }}"))?
            .syntax()
            .descendants()
            .find_map(ast::MacroCall::cast)
            .and_then(|call| call.token_tree())
            .ok_or("replacement macro has no token tree")?;
        let range = source_tree.syntax().text_range();
        if replacements
            .iter()
            .any(|(existing, _): &(ast::TokenTree, ast::TokenTree)| {
                let existing = existing.syntax().text_range();
                existing.start() <= range.start() && existing.end() >= range.end()
            })
        {
            continue;
        }
        replacements.retain(|(existing, _)| {
            let existing = existing.syntax().text_range();
            !(range.start() <= existing.start() && range.end() >= existing.end())
        });
        replacements.push((source_tree, replacement));
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

impl Scope<'_> {
    fn resolve(self, root: &SyntaxNode) -> Result<Option<SyntaxNode>, Box<dyn Error>> {
        match self {
            Self::Function(name) => function(root, name)
                .map(|function| function.map(|function| function.syntax().clone())),
            Self::Method { owner, name } => {
                let methods = methods(root, owner)
                    .filter(|function| {
                        function
                            .name()
                            .is_some_and(|candidate| candidate.text() == name)
                    })
                    .collect::<Vec<_>>();
                match methods.as_slice() {
                    [] => Ok(None),
                    [method] => Ok(Some(method.syntax().clone())),
                    _ => Err(format!("more than one method `{owner}::{name}`").into()),
                }
            }
        }
    }
}

fn retain_outermost(
    replacements: &mut Vec<(ast::TokenTree, ast::TokenTree)>,
    source: ast::TokenTree,
    replacement: ast::TokenTree,
) {
    let range = source.syntax().text_range();
    if replacements.iter().any(|(existing, _)| {
        let existing = existing.syntax().text_range();
        existing.start() <= range.start() && existing.end() >= range.end()
    }) {
        return;
    }
    replacements.retain(|(existing, _)| {
        let existing = existing.syntax().text_range();
        !(range.start() <= existing.start() && range.end() >= existing.end())
    });
    replacements.push((source, replacement));
}

fn add_use(source: &mut String, path: &str) -> Result<(), Box<dyn Error>> {
    let item = make::use_(
        std::iter::empty(),
        None,
        make::use_tree(make::path_from_text(path), None, None, false),
    );
    let (editor, root) = open(source)?;
    let anchor = root
        .children()
        .find(|node| ast::Item::can_cast(node.kind()))
        .ok_or("source has no items")?;
    editor.insert_all(
        Position::before(&anchor),
        vec![
            item.syntax().clone().into(),
            make::tokens::whitespace("\n").into(),
        ],
    );
    commit(source, editor)
}

fn use_tree_removal(tree: &ast::UseTree) -> Vec<SyntaxElement> {
    let mut elements = vec![tree.syntax().clone().into()];
    let mut after = Vec::new();
    let mut cursor = tree.syntax().next_sibling_or_token();
    while let Some(element) = cursor {
        cursor = element.next_sibling_or_token();
        match element.kind() {
            SyntaxKind::WHITESPACE => after.push(element),
            SyntaxKind::COMMA => {
                after.push(element);
                elements.extend(after);
                return elements;
            }
            _ => break,
        }
    }
    let mut before = Vec::new();
    let mut cursor = tree.syntax().prev_sibling_or_token();
    while let Some(element) = cursor {
        cursor = element.prev_sibling_or_token();
        match element.kind() {
            SyntaxKind::WHITESPACE => before.push(element),
            SyntaxKind::COMMA => {
                before.push(element);
                elements.extend(before);
                return elements;
            }
            _ => break,
        }
    }
    elements
}

fn open(source: &str) -> Result<(SyntaxEditor, SyntaxNode), Box<dyn Error>> {
    let file = parse(source)?;
    Ok(SyntaxEditor::new(file.syntax().clone()))
}

fn commit(source: &mut String, editor: SyntaxEditor) -> Result<(), Box<dyn Error>> {
    let output = editor.finish().new_root().to_string();
    parse(&output)?;
    *source = output;
    Ok(())
}

fn parse(source: &str) -> Result<SourceFile, Box<dyn Error>> {
    let parsed = SourceFile::parse(source, Edition::CURRENT);
    if parsed.errors().is_empty() {
        Ok(parsed.tree())
    } else {
        Err(format!("could not parse Rust source: {:?}", parsed.errors()).into())
    }
}

fn attr_targets(
    root: &SyntaxNode,
    target: AttrTarget<'_>,
) -> Result<Vec<SyntaxNode>, Box<dyn Error>> {
    let nodes: Vec<SyntaxNode> = match target {
        AttrTarget::Struct(name) => root
            .descendants()
            .filter_map(ast::Struct::cast)
            .filter(|item| item.name().is_some_and(|candidate| candidate.text() == name))
            .map(|item| item.syntax().clone())
            .collect(),
        AttrTarget::Enum(name) => root
            .descendants()
            .filter_map(ast::Enum::cast)
            .filter(|item| item.name().is_some_and(|candidate| candidate.text() == name))
            .map(|item| item.syntax().clone())
            .collect(),
        AttrTarget::Module(name) => root
            .descendants()
            .filter_map(ast::Module::cast)
            .filter(|item| item.name().is_some_and(|candidate| candidate.text() == name))
            .map(|item| item.syntax().clone())
            .collect(),
        AttrTarget::Method { owner, name } => methods(root, owner)
            .filter(|item| item.name().is_some_and(|candidate| candidate.text() == name))
            .map(|item| item.syntax().clone())
            .collect(),
        AttrTarget::Impl { owner, method } => root
            .descendants()
            .filter_map(ast::Impl::cast)
            .filter(|implementation| {
                implementation
                    .self_ty()
                    .is_some_and(|ty| ty.syntax().text() == owner)
                    && implementation
                        .assoc_item_list()
                        .into_iter()
                        .flat_map(|items| items.assoc_items())
                        .any(|item| {
                            matches!(item, ast::AssocItem::Fn(function) if function.name().is_some_and(|name| name.text() == method))
                        })
            })
            .map(|item| item.syntax().clone())
            .collect(),
    };
    if nodes.len() <= 1 {
        Ok(nodes)
    } else {
        Err("attribute target appears more than once".into())
    }
}

fn methods(root: &SyntaxNode, owner: &str) -> impl Iterator<Item = ast::Fn> {
    root.descendants()
        .filter_map(ast::Impl::cast)
        .filter(move |implementation| {
            implementation
                .self_ty()
                .is_some_and(|ty| ty.syntax().text() == owner)
        })
        .flat_map(|implementation| implementation.assoc_item_list())
        .flat_map(|items| items.assoc_items())
        .filter_map(|item| match item {
            ast::AssocItem::Fn(function) => Some(function),
            _ => None,
        })
        .collect::<Vec<_>>()
        .into_iter()
}

fn function(root: &SyntaxNode, name: &str) -> Result<Option<ast::Fn>, Box<dyn Error>> {
    let functions = root
        .descendants()
        .filter_map(ast::Fn::cast)
        .filter(|function| {
            function
                .name()
                .is_some_and(|candidate| candidate.text() == name)
                && function
                    .syntax()
                    .ancestors()
                    .find_map(ast::Impl::cast)
                    .is_none()
        })
        .collect::<Vec<_>>();
    match functions.as_slice() {
        [] => Ok(None),
        [function] => Ok(Some(function.clone())),
        _ => Err(format!("more than one function `{name}`").into()),
    }
}

fn method(root: &SyntaxNode, owner: &str, name: &str) -> Result<ast::Fn, Box<dyn Error>> {
    one(
        methods(root, owner).filter(|function| {
            function
                .name()
                .is_some_and(|candidate| candidate.text() == name)
        }),
        &format!("method `{owner}::{name}`"),
    )
}

fn named<N: AstNode + HasName>(root: &SyntaxNode, name: &str) -> Result<N, Box<dyn Error>> {
    one(
        root.descendants().filter_map(N::cast).filter(|item| {
            item.name()
                .is_some_and(|candidate| candidate.text() == name)
        }),
        name,
    )
}

fn one<T>(mut items: impl Iterator<Item = T>, description: &str) -> Result<T, Box<dyn Error>> {
    let item = items.next().ok_or_else(|| format!("no {description}"))?;
    if items.next().is_some() {
        return Err(format!("more than one {description}").into());
    }
    Ok(item)
}

fn visibility(source: &str) -> Result<ast::Visibility, Box<dyn Error>> {
    let file = parse(&format!("{source} fn replacement() {{}}"))?;
    file.syntax()
        .descendants()
        .find_map(ast::Visibility::cast)
        .ok_or_else(|| format!("invalid visibility `{source}`").into())
}

fn expression(source: &str) -> Result<ast::Expr, Box<dyn Error>> {
    let file = parse(&format!("fn replacement() {{ {source}; }}"))?;
    file.syntax()
        .descendants()
        .find_map(ast::ExprStmt::cast)
        .and_then(|statement| statement.expr())
        .ok_or_else(|| format!("invalid expression `{source}`").into())
}
