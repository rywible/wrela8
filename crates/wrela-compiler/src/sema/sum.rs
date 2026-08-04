use crate::sema::SemaError;
use crate::sema::bodies::{self, ModuleCtx};
use crate::sema::generics;
use crate::sema::types::{self, Type, TypeArg};
use crate::syntax::ast::Span;

fn no_span() -> Span {
    Span {
        line: 0,
        col: 0,
        ..Default::default()
    }
}

pub(crate) fn sum_ctors(
    ty: &Type,
    mctx: &ModuleCtx,
) -> Result<Vec<(String, Vec<Type>)>, SemaError> {
    match ty {
        Type::Option(inner) => Ok(vec![
            ("Some".to_string(), vec![(**inner).clone()]),
            ("None".to_string(), vec![]),
        ]),
        Type::Result(ok, err) => Ok(vec![
            ("Ok".to_string(), vec![(**ok).clone()]),
            ("Err".to_string(), vec![(**err).clone()]),
        ]),
        Type::Named(name, targs) if name == "CallError" => {
            let Some(TypeArg::Type(e_ty)) = targs.first() else {
                return Err(SemaError::at(
                    "type",
                    "`CallError` is missing its error argument".to_string(),
                    no_span(),
                ));
            };
            Ok(vec![
                ("Op".to_string(), vec![e_ty.clone()]),
                ("Cancelled".to_string(), vec![]),
                ("DeadlineExceeded".to_string(), vec![]),
                (
                    "NotAdmitted".to_string(),
                    vec![
                        Type::Named("Admission".to_string(), vec![]),
                        bodies::not_admitted_args_type(targs),
                    ],
                ),
            ])
        }
        Type::Named(name, targs)
            if targs.is_empty() && crate::sema::stdlib_enums::is_auto_visible(name) =>
        {
            let variants = crate::sema::stdlib_enums::variant_strs(name)?.ok_or_else(|| {
                SemaError::at("type", format!("`{name}` is not an enum"), no_span())
            })?;
            Ok(variants
                .iter()
                .map(|v| ((*v).to_string(), Vec::new()))
                .collect())
        }
        Type::Named(name, targs) => {
            let e = if targs.is_empty() {
                match mctx.enums.get(name) {
                    Some(e) => std::borrow::Cow::Borrowed(&e.decl),
                    None => {
                        return Err(SemaError::at(
                            "type",
                            format!("`{name}` is not an enum"),
                            no_span(),
                        ));
                    }
                }
            } else {
                match mctx.enums.get(name) {
                    Some(_) => std::borrow::Cow::Owned(generics::instantiate_enum(
                        mctx,
                        name,
                        targs,
                        no_span(),
                    )?),
                    None => {
                        return Err(SemaError::at(
                            "type",
                            format!("`{name}` is not an enum"),
                            no_span(),
                        ));
                    }
                }
            };
            Ok(e.variants
                .iter()
                .map(|v| (v.name.clone(), bodies::decl_variant_payload_types(v)))
                .collect())
        }
        other => Err(SemaError::at(
            "type",
            format!(
                "cannot match a variant pattern against type `{}`",
                types::render_type(other)
            ),
            no_span(),
        )),
    }
}
