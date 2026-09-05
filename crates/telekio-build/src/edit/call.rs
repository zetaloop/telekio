use std::error::Error;

use ra_ap_syntax::{
    AstNode,
    ast::{self, HasGenericArgs, make},
};

use super::{Call, Scope, call_arguments, count_targets, edit_scope, expression};

pub fn delegate_call(
    source: &mut String,
    scope: Scope<'_>,
    call: Call<'_>,
    helper: &str,
    context: &[&str],
) -> Result<(), Box<dyn Error>> {
    let count = count_targets(
        source,
        (scope, call),
        &|root, (scope, call)| {
            let Some(scope) = scope.resolve(root)? else {
                return Ok(0);
            };
            Ok(usize::from(call_arguments(&scope, call)?.is_some()))
        },
        &|(scope, call), tree, inner| scope.inside(tree).map(|scope| ((scope, call), inner)),
    )?;
    match count {
        0 => return Err("call was not found in selected scope".into()),
        1 => {}
        _ => return Err("call appears more than once in selected scope".into()),
    }
    if edit_scope(source, scope, &|editor, scope| {
        let Some(arguments) = call_arguments(scope, call)? else {
            return Ok(false);
        };
        let node = arguments
            .syntax()
            .parent()
            .ok_or("call has no expression")?;
        let mut inputs = context
            .iter()
            .map(|source| expression(source))
            .collect::<Result<Vec<_>, _>>()?;
        let generics = if let Some(method) = ast::MethodCallExpr::cast(node.clone()) {
            inputs.push(method.receiver().ok_or("method call has no receiver")?);
            method.generic_arg_list()
        } else {
            ast::CallExpr::cast(node.clone())
                .and_then(|call| call.expr())
                .and_then(|expression| match expression {
                    ast::Expr::PathExpr(path) => path.path(),
                    _ => None,
                })
                .and_then(|path| path.segment())
                .and_then(|segment| segment.generic_arg_list())
        };
        inputs.extend(arguments.args());
        let helper = generics.map_or_else(
            || helper.to_owned(),
            |generics| format!("{helper}{}", generics.syntax().text()),
        );
        let delegated = make::expr_call(expression(&helper)?, make::arg_list(inputs));
        editor.replace(node, delegated.syntax().clone());
        Ok(true)
    })? {
        Ok(())
    } else {
        Err("call was not found in selected scope".into())
    }
}
