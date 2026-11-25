//! rsbatis proc-macros: `where_!`、`#[derive(Entity)]`。
//!
//! 两个宏的分工：
//!
//! - `where_!` 只做**语法重写**：把 `User.age >= 18 && User.name.like("x%")` 这种
//!   在 Rust 里只能求值成 `bool` 的表达式，改写成构造 `Expr` 的方法调用链。
//!   类型检查仍然完全交给 `rustc`——列名错是 `E0599`，值类型错是 `E0277`。
//! - `#[derive(Entity)]` 只做**代码生成**：为结构体的每个字段生成 `col_<field>()`，
//!   供 `where_!` 重写 `User.age` 时调用。
//!
//! 两者必须配套：`User.age` 会被重写成 `User::col_age()`。
//!
//! ```ignore
//! #[derive(Entity)]
//! #[rsbatis(table = "users")]
//! struct User { id: i64, name: String, age: Option<i32> }
//!
//! let stmt = select::<User>()
//!     .where_(where_!(User.age >= 18 && User.name.like("zhang%")))
//!     .build(Dialect::MySql);
//! ```

extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::{TokenStream as TokenStream2, TokenTree};
use quote::{format_ident, quote};
use syn::{
    spanned::Spanned, BinOp as SynBinOp, Data, DeriveInput, Expr as SynExpr, Field, Fields,
    GenericArgument, LitStr, Member, PathArguments, Type, UnOp,
};

/// 宏展开时引用的门面包路径。proc-macro 没有 `$crate`，只能硬编码；
/// 用户若在 Cargo.toml 里重命名本依赖，展开会失败（serde 同样如此）。
const CORE: &str = "::rsbatis::__private";
/// `#[derive(Entity)]` 引用 trait 与 `Column` 的路径。
const CRATE: &str = "::rsbatis";

/// 允许在 `where_!` 里直接写的 SQL 函数。
///
/// 白名单而不是"任何函数名都当 SQL 函数"：宏无法知道 `foo()` 是用户的 Rust 函数还是
/// 数据库函数，猜错了的分寸是"生成的 SQL 在运行时才发现函数不存在"。
const SQL_FUNCTIONS: &[&str] = &[
    "now", "coalesce", "lower", "upper", "abs", "length", "trim", "count", "sum", "avg", "min",
    "max", "substr", "ifnull", "nullif",
];

// ============================================================
// where_!
// ============================================================

#[proc_macro]
pub fn where_(input: TokenStream) -> TokenStream {
    let expr = match syn::parse::<SynExpr>(input) {
        Ok(e) => e,
        Err(err) => return err.to_compile_error().into(),
    };
    let core: TokenStream2 = CORE.parse().expect("CORE 是合法路径");
    match rewrite(&expr, &core, &Aliases::new()) {
        Ok(node) => node.into_expr(&core).into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// 别名表：`select! { User as u }` 里的 `u` -> 实体 `User`。
///
/// 必须在**重写列引用之前**就收集齐：`u.age` 只有在知道 `u` 是 `User` 时才可能
/// 变成 `User::col_age().of_table("u")`。
type Aliases = std::collections::HashMap<String, syn::Ident>;

/// 重写过程中的三种中间形态。
///
/// 区分它们不是为了好看，而是为了保住类型检查：只有当左操作数是**列**、右操作数是**值**时，
/// 才能走 `Column::ge(impl IntoValue<T>)` 这条路径，让 rustc 用 `T` 校验右侧类型。
/// 如果一律走 `Expr::cmp(Expr, Expr)`，`User.age == "x"` 会被 `Expr::lit("x")` 悄悄接受，
/// 类型安全就丢了。
enum Node {
    /// 一个具体列，展开成 `User::col_age()`，类型是 `Column<T>`。
    Col(TokenStream2),
    /// 已经是 `Expr`。
    Expr(TokenStream2),
    /// 一个 Rust 值表达式（字面量、局部变量、常量）。
    Val(TokenStream2),
}

impl Node {
    fn into_expr(self, core: &TokenStream2) -> TokenStream2 {
        match self {
            // Column::expr() 消耗 self，转成 Expr::Col
            Node::Col(ts) => quote!((#ts).expr()),
            Node::Expr(ts) => ts,
            Node::Val(ts) => quote!(#core::Expr::lit(#ts)),
        }
    }
}

fn rewrite(expr: &SynExpr, core: &TokenStream2, al: &Aliases) -> syn::Result<Node> {
    match expr {
        // `(a && b) || c` —— 用户手写的括号必须保留，不能靠优先级去猜
        SynExpr::Paren(p) => {
            let inner = rewrite(&p.expr, core, al)?.into_expr(core);
            Ok(Node::Expr(quote!(#core::Expr::paren(#inner))))
        }

        // `User.age` -> `User::col_age()`
        SynExpr::Field(f) => {
            let base = single_path_ident(&f.base).ok_or_else(|| {
                syn::Error::new_spanned(
                    &f.base,
                    "rsbatis: 列引用必须是 `实体.字段` 形式，实体名必须是单个标识符（如 `User.age`）",
                )
            })?;
            let Member::Named(field) = &f.member else {
                return Err(syn::Error::new_spanned(
                    &f.member,
                    "rsbatis: 列引用不支持元组下标（`.0`），只能写字段名",
                ));
            };
            let col_fn = format_ident!("col_{}", field);

            // 别名优先：`select! { User as u ... }` 里的 `u.age`
            // -> `User::col_age().of_table("u")`
            if let Some(ent) = al.get(&base) {
                let alias_lit = syn::LitStr::new(&base, f.base.span());
                return Ok(Node::Col(quote!(#ent::#col_fn().of_table(#alias_lit))));
            }

            let first_upper = base
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false);
            if !first_upper {
                return Err(syn::Error::new_spanned(
                    &f.base,
                    format!(
                        "rsbatis: `{base}` 不是实体类型名。列引用必须写成 `User.age` 且实体名首字母大写；\
                         局部变量的字段不参与 SQL 重写"
                    ),
                ));
            }
            // base 是 String；直接插进 quote! 会变成字符串字面量（`"User"::col_age()`）。
            // 用原始表达式 token 才能得到合法的 `User::col_age()`。
            let base_path = &f.base;
            Ok(Node::Col(quote!(#base_path::#col_fn())))
        }

        SynExpr::Binary(b) => rewrite_binary(b, core, al),

        SynExpr::Unary(u) => {
            let inner = rewrite(&u.expr, core, al)?.into_expr(core);
            match u.op {
                UnOp::Not(_) => Ok(Node::Expr(quote!(#core::Expr::not(#inner)))),
                UnOp::Neg(_) => Ok(Node::Expr(quote!(#core::Expr::neg(#inner)))),
                _ => Err(syn::Error::new_spanned(
                    u,
                    "rsbatis: 只支持 `!` 与 `-` 两个一元运算符",
                )),
            }
        }

        // `User.name.like("x%")` -> `User::col_name().like("x%")`
        SynExpr::MethodCall(m) => {
            let recv = match rewrite(&m.receiver, core, al)? {
                Node::Col(ts) | Node::Expr(ts) | Node::Val(ts) => ts,
            };
            let method = &m.method;
            let args: Vec<TokenStream2> = m.args.iter().map(|a| quote!(#a)).collect();
            Ok(Node::Expr(quote!((#recv).#method(#(#args),*))))
        }

        // `coalesce(User.name, "x")` 这类 SQL 函数调用
        SynExpr::Call(c) => {
            let name = single_path_ident(&c.func).ok_or_else(|| {
                syn::Error::new_spanned(
                    &c.func,
                    "rsbatis: 函数调用必须是单个标识符（如 `coalesce(...)`）",
                )
            })?;
            if !SQL_FUNCTIONS.contains(&name.as_str()) {
                return Err(syn::Error::new_spanned(
                    c,
                    format!(
                        "rsbatis: `{name}()` 不在已知 SQL 函数白名单内。\
                         白名单：{}。自定义函数请用 `Expr::func(\"{name}\", args)` 显式构造",
                        SQL_FUNCTIONS.join(", ")
                    ),
                ));
            }
            let args: Vec<TokenStream2> = c
                .args
                .iter()
                .map(|a| rewrite(a, core, al).map(|n| n.into_expr(core)))
                .collect::<syn::Result<Vec<_>>>()?;
            Ok(Node::Expr(
                quote!(#core::Expr::func(#name, vec![#(#args),*])),
            ))
        }

        SynExpr::Lit(_) | SynExpr::Path(_) => Ok(Node::Val(quote!(#expr))),

        _ => Err(syn::Error::new_spanned(
            expr,
            "rsbatis: where_! 不支持的表达式。支持：比较、`&&`/`||`/`!`、`+ - * / %`、\
             括号、`实体.字段`、列方法链（`.like/.in_/.is_null` 等）、SQL 函数白名单、字面量",
        )),
    }
}

fn rewrite_binary(b: &syn::ExprBinary, core: &TokenStream2, al: &Aliases) -> syn::Result<Node> {
    match b.op {
        SynBinOp::And(_) | SynBinOp::Or(_) => {
            let is_and = matches!(b.op, SynBinOp::And(_));
            let mut leaves: Vec<&SynExpr> = Vec::new();
            flatten_bool(&b.left, is_and, &mut leaves);
            flatten_bool(&b.right, is_and, &mut leaves);
            let parts: Vec<TokenStream2> = leaves
                .iter()
                .map(|e| rewrite(e, core, al).map(|n| n.into_expr(core)))
                .collect::<syn::Result<Vec<_>>>()?;
            let ctor = if is_and {
                quote!(#core::Expr::and)
            } else {
                quote!(#core::Expr::or)
            };
            Ok(Node::Expr(quote!(#ctor(vec![#(#parts),*]))))
        }

        SynBinOp::Eq(_)
        | SynBinOp::Ne(_)
        | SynBinOp::Gt(_)
        | SynBinOp::Ge(_)
        | SynBinOp::Lt(_)
        | SynBinOp::Le(_) => rewrite_compare(b, core, al),

        SynBinOp::Add(_)
        | SynBinOp::Sub(_)
        | SynBinOp::Mul(_)
        | SynBinOp::Div(_)
        | SynBinOp::Rem(_) => {
            let left = rewrite(&b.left, core, al)?.into_expr(core);
            let right = rewrite(&b.right, core, al)?.into_expr(core);
            let op = arith_variant(&b.op);
            Ok(Node::Expr(
                quote!(#core::Expr::arith(#core::ArithOp::#op, #left, #right)),
            ))
        }

        _ => Err(syn::Error::new_spanned(
            b,
            "rsbatis: 不支持该二元运算符（位运算、比较链请改写）",
        )),
    }
}

fn rewrite_compare(b: &syn::ExprBinary, core: &TokenStream2, al: &Aliases) -> syn::Result<Node> {
    // `User.age == null` / `!= null`：SQL 里必须变成 IS NULL / IS NOT NULL，
    // 因为没有 `= NULL` 这种能返回 true 的比较。
    if is_null_marker(&b.right) {
        let left = match rewrite(&b.left, core, al)? {
            Node::Col(ts) => ts,
            _ => {
                return Err(syn::Error::new_spanned(
                    &b.left,
                    "rsbatis: `IS NULL` 左侧必须是实体列（如 `User.age == null`）",
                ))
            }
        };
        let m = match b.op {
            SynBinOp::Eq(_) => format_ident!("is_null"),
            SynBinOp::Ne(_) => format_ident!("is_not_null"),
            _ => {
                return Err(syn::Error::new_spanned(
                    b,
                    "rsbatis: `null` 只能与 `==` / `!=` 搭配",
                ))
            }
        };
        return Ok(Node::Expr(quote!((#left).#m())));
    }

    let left = rewrite(&b.left, core, al)?;
    let right = rewrite(&b.right, core, al)?;

    match (left, right) {
        // 列 vs 值：走 Column 方法，让 IntoValue<T> 在编译期校验右侧类型
        (Node::Col(l), Node::Val(r)) => {
            let m = col_method(&b.op);
            Ok(Node::Expr(quote!((#l).#m(#r))))
        }
        // 列 vs 列：只有 eq 有对应的 Column 方法
        (Node::Col(l), Node::Col(r)) => match b.op {
            SynBinOp::Eq(_) => Ok(Node::Expr(quote!((#l).eq_col(#r)))),
            _ => {
                let l = quote!((#l).expr());
                let r = quote!((#r).expr());
                let variant = binop_variant(&b.op);
                Ok(Node::Expr(
                    quote!(#core::Expr::cmp(#core::BinOp::#variant, #l, #r)),
                ))
            }
        },
        // 其他组合（函数比常量、表达式比表达式）：在 Expr 层比较
        (l, r) => {
            let l = l.into_expr(core);
            let r = r.into_expr(core);
            let variant = binop_variant(&b.op);
            Ok(Node::Expr(
                quote!(#core::Expr::cmp(#core::BinOp::#variant, #l, #r)),
            ))
        }
    }
}

fn flatten_bool<'a>(e: &'a SynExpr, is_and: bool, out: &mut Vec<&'a SynExpr>) {
    if let SynExpr::Binary(b) = e {
        let same = matches!(
            (&b.op, is_and),
            (SynBinOp::And(_), true) | (SynBinOp::Or(_), false)
        );
        if same {
            flatten_bool(&b.left, is_and, out);
            flatten_bool(&b.right, is_and, out);
            return;
        }
    }
    out.push(e);
}

fn single_path_ident(e: &SynExpr) -> Option<String> {
    let SynExpr::Path(p) = e else { return None };
    if p.path.segments.len() != 1 {
        return None;
    }
    let seg = p.path.segments.first()?;
    if seg.arguments.is_none() {
        Some(seg.ident.to_string())
    } else {
        None
    }
}

/// `null` / `None` 作为右操作数时按 SQL NULL 处理。
fn is_null_marker(e: &SynExpr) -> bool {
    matches!(single_path_ident(e).as_deref(), Some("null") | Some("None"))
}

fn col_method(op: &SynBinOp) -> syn::Ident {
    match op {
        SynBinOp::Eq(_) => format_ident!("eq"),
        SynBinOp::Ne(_) => format_ident!("ne"),
        SynBinOp::Gt(_) => format_ident!("gt"),
        SynBinOp::Ge(_) => format_ident!("ge"),
        SynBinOp::Lt(_) => format_ident!("lt"),
        SynBinOp::Le(_) => format_ident!("le"),
        _ => unreachable!("只有比较运算符会走到这里"),
    }
}

fn binop_variant(op: &SynBinOp) -> syn::Ident {
    match op {
        SynBinOp::Eq(_) => format_ident!("Eq"),
        SynBinOp::Ne(_) => format_ident!("Ne"),
        SynBinOp::Gt(_) => format_ident!("Gt"),
        SynBinOp::Ge(_) => format_ident!("Ge"),
        SynBinOp::Lt(_) => format_ident!("Lt"),
        SynBinOp::Le(_) => format_ident!("Le"),
        _ => unreachable!("只有比较运算符会走到这里"),
    }
}

fn arith_variant(op: &SynBinOp) -> syn::Ident {
    match op {
        SynBinOp::Add(_) => format_ident!("Add"),
        SynBinOp::Sub(_) => format_ident!("Sub"),
        SynBinOp::Mul(_) => format_ident!("Mul"),
        SynBinOp::Div(_) => format_ident!("Div"),
        SynBinOp::Rem(_) => format_ident!("Mod"),
        _ => unreachable!("只有算术运算符会走到这里"),
    }
}

// ============================================================
// #[derive(Entity)]
// ============================================================
//
// 目标：把 `User.age` 背后的机械劳动消掉。
//
// ```ignore
// #[derive(Entity)]
// #[rsbatis(table = "users")]
// pub struct User {
//     pub id: i64,
//     pub name: String,
//     pub age: Option<i32>,   // Option<T> => 列可空，col_age() 仍是 Column<i32>
// }
// ```
//
// 展开出：
// - `impl Entity`：TABLE / COLUMNS / NULLABLE
// - `impl User { pub fn col_id() -> Column<i64>, col_name() -> Column<String>, ... }`
//
// 注意 `col_age()` 返回 `Column<i32>` 而不是 `Column<Option<i32>>`：比较
// `age().lt(18)` 的类型参数不该被可空性污染。是否可空由 `Entity::NULLABLE`
// 单独表达，`set_null(col_age())` 负责写入 NULL。

#[proc_macro_derive(Entity, attributes(rsbatis))]
pub fn derive_entity(input: TokenStream) -> TokenStream {
    let ast = match syn::parse::<DeriveInput>(input) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };
    match expand_entity(&ast) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand_entity(ast: &DeriveInput) -> syn::Result<TokenStream2> {
    // CRATE 是字符串常量；quote! 会把 `#CRATE` 直接插成字符串字面量
    // （生成 `"::rsbatis"::Entity` 这种无效代码），必须先解析成 token 流。
    let krate: TokenStream2 = CRATE.parse().expect("CRATE 是合法路径");
    let table = entity_table(ast)?;

    let data = match &ast.data {
        Data::Struct(s) => s,
        _ => {
            return Err(syn::Error::new_spanned(
                ast,
                "rsbatis: #[derive(Entity)] 只能用于具名结构体 `struct X { .. }`；\
                 元组结构体和枚举没有列的概念",
            ))
        }
    };
    let named = match &data.fields {
        Fields::Named(f) => &f.named,
        _ => {
            return Err(syn::Error::new_spanned(
                &data.fields,
                "rsbatis: #[derive(Entity)] 需要具名字段，元组结构体不支持",
            ))
        }
    };

    let mut col_names: Vec<String> = Vec::new();
    let mut null_names: Vec<String> = Vec::new();
    let mut pk_names: Vec<String> = Vec::new();
    let mut col_fns: Vec<TokenStream2> = Vec::new();
    let mut value_pushes: Vec<TokenStream2> = Vec::new();

    for f in named {
        let Some(ident) = f.ident.as_ref() else {
            return Err(syn::Error::new_spanned(f, "rsbatis: 字段缺少名字"));
        };
        let (col, is_pk) = field_attrs(f, ident)?;
        let (inner, is_nullable) = map_field_type(&f.ty)?;

        let col_lit = LitStr::new(&col, ident.span());
        let col_fn = format_ident!("col_{}", ident);
        let fn_doc = format!(
            "本字段对应的列 `{col}`。由 `#[derive(Entity)]` 生成{}",
            if is_nullable {
                "（该列可空）"
            } else {
                ""
            }
        );

        col_fns.push(quote! {
            #[doc = #fn_doc]
            #[allow(dead_code)]
            pub fn #col_fn() -> #krate::Column<#inner> {
                #krate::Column::new(<Self as #krate::Entity>::TABLE, #col_lit)
            }
        });

        // 对象式插入：把字段值收集成 (列名, Value)。
        // 可空字段为 None 时不收集该列——交给数据库默认值，而不是写 NULL。
        if is_nullable {
            value_pushes.push(quote! {
                if let ::core::option::Option::Some(__rsbatis_v) = &self.#ident {
                    __out.push((
                        #col_lit,
                        <_ as #krate::IntoValue<#inner>>::into_value(::core::clone::Clone::clone(__rsbatis_v)),
                    ));
                }
            });
        } else {
            value_pushes.push(quote! {
                __out.push((
                    #col_lit,
                    <_ as #krate::IntoValue<#inner>>::into_value(::core::clone::Clone::clone(&self.#ident)),
                ));
            });
        }

        if is_pk {
            pk_names.push(col.clone());
        }
        col_names.push(col.clone());
        if is_nullable {
            null_names.push(col);
        }
    }

    let table_lit = LitStr::new(&table, ast.ident.span());
    let col_lits: Vec<LitStr> = col_names
        .iter()
        .map(|c| LitStr::new(c, ast.ident.span()))
        .collect();
    let null_lits: Vec<LitStr> = null_names
        .iter()
        .map(|c| LitStr::new(c, ast.ident.span()))
        .collect();
    let pk_lits: Vec<LitStr> = pk_names
        .iter()
        .map(|c| LitStr::new(c, ast.ident.span()))
        .collect();

    let name = &ast.ident;
    let (impl_g, ty_g, where_c) = ast.generics.split_for_impl();

    Ok(quote! {
        impl #impl_g #krate::Entity for #name #ty_g #where_c {
            const TABLE: &'static str = #table_lit;
            const COLUMNS: &'static [&'static str] = &[#(#col_lits),*];
            const NULLABLE: &'static [&'static str] = &[#(#null_lits),*];
            const PRIMARY_KEY: &'static [&'static str] = &[#(#pk_lits),*];

            fn column_values(
                &self,
                __out: &mut ::std::vec::Vec<(&'static str, #krate::Value)>,
            ) {
                #(#value_pushes)*
            }
        }

        impl #impl_g #name #ty_g #where_c {
            #(#col_fns)*
        }
    })
}

/// 表名：`#[rsbatis(table = "x")]`，没有就用结构体名的 snake_case。
fn entity_table(ast: &DeriveInput) -> syn::Result<String> {
    for attr in &ast.attrs {
        if !attr.path().is_ident("rsbatis") {
            continue;
        }
        let mut table: Option<String> = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("table") {
                let v: LitStr = meta.value()?.parse()?;
                table = Some(v.value());
                Ok(())
            } else {
                Err(meta.error(
                    "rsbatis: 结构体级属性只支持 `table = \"...\"` 与 `column = \"...\"`（后者仅字段级）",
                ))
            }
        })?;
        if let Some(t) = table {
            return Ok(t);
        }
    }
    Ok(to_snake_case(&ast.ident.to_string()))
}

/// 列名：`#[rsbatis(column = "x")]`，没有就用字段名原文。
fn field_column(f: &Field, ident: &syn::Ident) -> syn::Result<String> {
    Ok(field_attrs(f, ident)?.0)
}

/// 字段级属性：`(列名, 是否主键)`。
fn field_attrs(f: &Field, ident: &syn::Ident) -> syn::Result<(String, bool)> {
    let mut col: Option<String> = None;
    let mut is_pk = false;
    for attr in &f.attrs {
        if !attr.path().is_ident("rsbatis") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("column") {
                let v: LitStr = meta.value()?.parse()?;
                col = Some(v.value());
                Ok(())
            } else if meta.path.is_ident("primary_key") {
                is_pk = true;
                Ok(())
            } else {
                Err(meta.error(
                    "rsbatis: 字段级属性只支持 `column = \"...\"` 与 `primary_key`；若想忽略该字段请删除字段",
                ))
            }
        })?;
    }
    Ok((col.unwrap_or_else(|| ident.to_string()), is_pk))
}

/// 字段类型 → (用作 Column 类型参数的类型, 是否可空)。
///
/// 白名单而非"任意类型"是有意的：ORM 必须知道一个 Rust 类型落库后变成什么列，
/// 猜错了会在驱动回读时变成 `TypeMismatch`，那时候离出错点已经很远了。
fn map_field_type(ty: &Type) -> syn::Result<(TokenStream2, bool)> {
    if let Some(inner) = option_inner(ty) {
        let (ts, _) = map_field_type(inner)?;
        return Ok((ts, true));
    }
    let name = type_name(ty);
    match name.as_str() {
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "f32" | "f64" | "bool"
        | "String" => Ok((quote!(#ty), false)),
        "Vec<u8>" => Ok((quote!(::std::vec::Vec<u8>), false)),
        // chrono 的日期时间：允许写 `NaiveDateTime` 或 `chrono::NaiveDateTime`
        _ if name == "NaiveDateTime" || name.ends_with("::NaiveDateTime") => {
            Ok((quote!(#ty), false))
        }
        other => Err(syn::Error::new_spanned(
            ty,
            format!(
                "rsbatis: 字段类型 `{other}` 没有对应的 SQL 列映射。\
                 当前支持 i8..i64 / u8..u64 / f32 / f64 / bool / String / Vec<u8> / NaiveDateTime（需开 chrono feature），\
                 可空请写成 Option<T>"
            ),
        )),
    }
}

fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    match args.args.first()? {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    }
}

/// 打印类型并去掉空白，用来做白名单比较（`Vec < u8 >` → `Vec<u8>`）。
fn type_name(ty: &Type) -> String {
    quote!(#ty).to_string().replace(' ', "")
}

fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.char_indices() {
        if ch.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

// ============================================================
// select!
// ============================================================
//
// 把 SQLAlchemy 的书写体验压到一个宏里：
//
// ```ignore
// let stmt = select! { User where User.age >= 18 order_by User.age desc limit 10 };
// ```
//
// 语法（段顺序固定，各段可省略）：
//
// ```text
// select! { [distinct] <Entity> [where <expr>] [order_by <expr> [asc|desc] (, <expr> [asc|desc])*]
//            [limit <int>] [offset <int>] }
// ```
//
// 各段的分界靠顶层标识符（`where` / `order_by` / `limit` / `offset` / `distinct`）识别。
// 括号/数组内部的 token 不参与分界判断（Group 是一个原子 token）。
// 代价是这几个词在 `select!` 内部成为保留字，不能在表达式里当普通变量名。

/// 字面量式插入：`insert! { User { name: "张三", age: 20 } }`。
///
/// 展开成 `insert::<User>().set(User::col_name(), "张三").set(User::col_age(), 20)`。
/// 关键点是**列名不在这里硬编码**：走 `col_<field>()`，所以
/// `#[rsbatis(column = "...")]` 的重命名、以及 `Option<T>` 列的类型参数，
/// 都由 derive 保证，宏只负责把字段名接上去。
///
/// 不写的字段就是不写该列（交给数据库默认值）；`Option` 字段想写 NULL 请用
/// `set_null(col_age())`，因为 `age: None` 在这里没有意义（等价于不写）。
#[proc_macro]
pub fn insert(input: TokenStream) -> TokenStream {
    let krate: TokenStream2 = CRATE.parse().expect("CRATE 是合法路径");
    let expr = match syn::parse::<syn::ExprStruct>(input) {
        Ok(e) => e,
        Err(e) => return e.to_compile_error().into(),
    };

    if expr.rest.is_some() {
        return syn::Error::new_spanned(
            &expr.rest,
            "rsbatis: `insert!` 不支持 `..base` 展开写法，请把要写的字段列全",
        )
        .to_compile_error()
        .into();
    }
    if expr.fields.is_empty() {
        return syn::Error::new_spanned(&expr.path, "rsbatis: `insert!` 至少要写一个字段")
            .to_compile_error()
            .into();
    }

    let entity = &expr.path;
    let mut setters: Vec<TokenStream2> = Vec::new();
    for f in &expr.fields {
        let member = &f.member;
        let ident = match member {
            syn::Member::Named(i) => i,
            syn::Member::Unnamed(_) => {
                return syn::Error::new_spanned(member, "rsbatis: `insert!` 需要具名字段")
                    .to_compile_error()
                    .into()
            }
        };
        let col_fn = format_ident!("col_{}", ident);
        let value = &f.expr;
        setters.push(quote! { .set(<#entity>::#col_fn(), #value) });
    }

    quote! { #krate::insert::<#entity>() #(#setters)* }.into()
}

#[proc_macro]
pub fn select(input: TokenStream) -> TokenStream {
    match expand_select(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand_select(input: TokenStream) -> syn::Result<TokenStream2> {
    let krate: TokenStream2 = CRATE.parse().expect("CRATE 是合法路径");
    let core: TokenStream2 = CORE.parse().expect("CORE 是合法路径");

    // input 是 proc_macro::TokenStream，必须先转成 proc_macro2 再拆 token
    let toks: Vec<TokenTree> = TokenStream2::from(input).into_iter().collect();
    if toks.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "rsbatis: select! 至少需要一个实体名，例如 `select! { User }`",
        ));
    }

    let mut pos = 0usize;
    let mut distinct = false;
    if is_ident(toks.first(), "distinct") {
        distinct = true;
        pos = 1;
    }

    let entity = match toks.get(pos) {
        Some(TokenTree::Ident(i)) => i.clone(),
        _ => return Err(syn::Error::new(
            toks.get(pos)
                .map(token_span)
                .unwrap_or_else(proc_macro2::Span::call_site),
            "rsbatis: select! 的第一段必须是实体名（首字母大写的类型），例如 `select! { User }`",
        )),
    };
    pos += 1;

    // 主表别名：`select! { User as u ... }`
    let mut main_alias: Option<String> = None;
    if is_ident(toks.get(pos), "as") {
        let as_span = toks
            .get(pos)
            .map(token_span)
            .unwrap_or_else(proc_macro2::Span::call_site);
        match toks.get(pos + 1) {
            Some(TokenTree::Ident(a)) => {
                check_alias_name(a)?;
                main_alias = Some(a.to_string());
                pos += 2;
            }
            _ => {
                return Err(syn::Error::new(
                    as_span,
                    "rsbatis: `as` 后面必须是别名（小写标识符），例如 `User as u`",
                ))
            }
        }
    }

    let mut where_toks: Vec<TokenTree> = Vec::new();
    let mut order_toks: Vec<TokenTree> = Vec::new();
    let mut limit_lit: Option<TokenTree> = None;
    let mut offset_lit: Option<TokenTree> = None;
    // 已结算的连接：(实体, 连接类型, on 段的 token)
    let mut joins: Vec<(syn::Ident, &'static str, Option<String>, Vec<TokenTree>)> = Vec::new();
    let mut pending_join: Option<(syn::Ident, &'static str, Option<String>)> = None;
    let mut join_on_toks: Vec<TokenTree> = Vec::new();
    let mut next_join_kind: Option<&'static str> = None;
    let mut seg = "";

    while pos < toks.len() {
        // 收 join 的实体名：`join Order on ...` 里的 `Order`
        if seg == "join_entity" {
            match &toks[pos] {
                TokenTree::Ident(e) => {
                    let kind = next_join_kind.take().unwrap_or("Inner");
                    let mut alias: Option<String> = None;
                    // `join Order as o on ...`
                    if is_ident(toks.get(pos + 1), "as") {
                        match toks.get(pos + 2) {
                            Some(TokenTree::Ident(a)) => {
                                check_alias_name(a)?;
                                alias = Some(a.to_string());
                                pos += 2;
                            }
                            _ => {
                                return Err(syn::Error::new(
                                    token_span(&toks[pos + 1]),
                                    "rsbatis: `as` 后面必须是别名（小写标识符），例如 `join Order as o on ...`",
                                ))
                            }
                        }
                    }
                    pending_join = Some((e.clone(), kind, alias));
                    seg = "";
                }
                _ => {
                    return Err(syn::Error::new(
                        token_span(&toks[pos]),
                        "rsbatis: `join` / `left_join` 后面必须是实体类型名（首字母大写），\
                         例如 `join Order on Order.user_id == User.id`",
                    ))
                }
            }
            pos += 1;
            continue;
        }

        if let TokenTree::Ident(i) = &toks[pos] {
            // 段关键字只在"独立出现"时才算边界。`Event.limit` 里的 limit 是列名：
            // 表里真有一列叫 limit / offset / where / order_by / on 时，若照旧切段，
            // 用户的查询会被切成两半，报出来的错误还完全指向别处。
            let after_dot = matches!(
                pos.checked_sub(1).map(|q| &toks[q]),
                Some(TokenTree::Punct(pd)) if pd.as_char() == '.'
            );
            if !after_dot {
                match i.to_string().as_str() {
                    "distinct" => {
                        distinct = true;
                        pos += 1;
                        continue;
                    }
                    "join" | "left_join" => {
                        flush_join(&mut pending_join, &mut join_on_toks, &mut joins, i.span())?;
                        next_join_kind = Some(if i == "join" { "Inner" } else { "Left" });
                        seg = "join_entity";
                        pos += 1;
                        continue;
                    }
                    "on" => {
                        if pending_join.is_none() {
                            return Err(syn::Error::new(
                                i.span(),
                                "rsbatis: `on` 必须紧跟在 `join <Entity>` 之后",
                            ));
                        }
                        seg = "join_on";
                        pos += 1;
                        continue;
                    }
                    "where" => {
                        flush_join(&mut pending_join, &mut join_on_toks, &mut joins, i.span())?;
                        if seg == "where" {
                            return Err(syn::Error::new(i.span(), "rsbatis: `where` 出现了两次"));
                        }
                        seg = "where";
                        pos += 1;
                        continue;
                    }
                    "order_by" => {
                        flush_join(&mut pending_join, &mut join_on_toks, &mut joins, i.span())?;
                        if seg == "order_by" {
                            return Err(syn::Error::new(
                                i.span(),
                                "rsbatis: `order_by` 出现了两次",
                            ));
                        }
                        seg = "order_by";
                        pos += 1;
                        continue;
                    }
                    "limit" => {
                        flush_join(&mut pending_join, &mut join_on_toks, &mut joins, i.span())?;
                        if limit_lit.is_some() {
                            return Err(syn::Error::new(i.span(), "rsbatis: `limit` 出现了两次"));
                        }
                        seg = "limit";
                        pos += 1;
                        continue;
                    }
                    "offset" => {
                        flush_join(&mut pending_join, &mut join_on_toks, &mut joins, i.span())?;
                        if offset_lit.is_some() {
                            return Err(syn::Error::new(i.span(), "rsbatis: `offset` 出现了两次"));
                        }
                        seg = "offset";
                        pos += 1;
                        continue;
                    }
                    _ => {}
                }
            }
        }
        match seg {
            "join_on" => join_on_toks.push(toks[pos].clone()),
            "where" => where_toks.push(toks[pos].clone()),
            "order_by" => order_toks.push(toks[pos].clone()),
            "limit" => {
                limit_lit = Some(toks[pos].clone());
                seg = "";
            }
            "offset" => {
                offset_lit = Some(toks[pos].clone());
                seg = "";
            }
            _ => {
                return Err(syn::Error::new(
                    token_span(&toks[pos]),
                    "rsbatis: select! 里出现了无法归段的 token。\
                     支持的段：`join <Entity> on <expr>` / `where <expr>` / \
                     `order_by <expr> [asc|desc]` / `limit <int>` / `offset <int>`",
                ))
            }
        }
        pos += 1;
    }
    flush_join(
        &mut pending_join,
        &mut join_on_toks,
        &mut joins,
        toks.last()
            .map(token_span)
            .unwrap_or_else(proc_macro2::Span::call_site),
    )?;

    // 别名表必须在重写任何列引用之前建好。
    let mut aliases: Aliases = Aliases::new();
    if let Some(a) = &main_alias {
        aliases.insert(a.clone(), entity.clone());
    }
    for (ent, _, alias, _) in &joins {
        if let Some(a) = alias {
            let sp = ent.span();
            if aliases.insert(a.clone(), ent.clone()).is_some() {
                return Err(syn::Error::new(
                    sp,
                    format!("rsbatis: 别名 `{a}` 重复。同一别名只能指向一个实体"),
                ));
            }
        }
    }

    let join_stmts = {
        let mut out: Vec<TokenStream2> = Vec::new();
        for (ent, kind, alias, on) in &joins {
            let on_expr: SynExpr = syn::parse2(toks_to_stream(on.clone()))
                .map_err(|e| with_hint(e, "rsbatis: `join ... on` 之后不是合法的 Rust 表达式"))?;
            let expr = rewrite(&on_expr, &core, &aliases)?.into_expr(&core);
            let kind = format_ident!("{kind}");
            out.push(match alias {
                Some(a) => {
                    let lit = syn::LitStr::new(a, ent.span());
                    quote!(
                        __rsbatis_stmt = __rsbatis_stmt.join_as::<#ent>(
                            #core::JoinKind::#kind, #lit, #expr
                        );
                    )
                }
                None => quote!(
                    __rsbatis_stmt = __rsbatis_stmt.join::<#ent>(#core::JoinKind::#kind, #expr);
                ),
            });
        }
        out
    };

    let where_expr = if where_toks.is_empty() {
        None
    } else {
        let e: SynExpr = syn::parse2(toks_to_stream(where_toks))
            .map_err(|e| with_hint(e, "rsbatis: where 段不是合法的 Rust 表达式"))?;
        let node = rewrite(&e, &core, &aliases)
            .map_err(|e| with_hint(e, "rsbatis: where 段无法转成 SQL 表达式"))?;
        Some(node.into_expr(&core))
    };

    let mut orders: Vec<(TokenStream2, TokenStream2)> = Vec::new();
    for group in split_commas(order_toks) {
        if group.is_empty() {
            continue;
        }
        let mut g = group;
        let dir = if let Some(TokenTree::Ident(i)) = g.last() {
            match i.to_string().as_str() {
                "desc" => {
                    g.pop();
                    quote!(#krate::Order::Desc)
                }
                "asc" => {
                    g.pop();
                    quote!(#krate::Order::Asc)
                }
                _ => quote!(#krate::Order::Asc),
            }
        } else {
            quote!(#krate::Order::Asc)
        };
        let e: SynExpr = syn::parse2(toks_to_stream(g))
            .map_err(|e| with_hint(e, "rsbatis: order_by 段不是合法的 Rust 表达式"))?;
        let node = rewrite(&e, &core, &aliases)
            .map_err(|e| with_hint(e, "rsbatis: order_by 段无法转成 SQL 表达式"))?;
        orders.push((node.into_expr(&core), dir));
    }

    let limit = parse_u64_lit(limit_lit, "limit")?;
    let offset = parse_u64_lit(offset_lit, "offset")?;

    let main_alias_stmt = match &main_alias {
        Some(a) => {
            let lit = syn::LitStr::new(a, entity.span());
            quote!(__rsbatis_stmt = __rsbatis_stmt.from_alias(#lit);)
        }
        None => quote!(),
    };
    let distinct_stmt = if distinct {
        quote!(__rsbatis_stmt = __rsbatis_stmt.distinct();)
    } else {
        quote!()
    };
    let where_stmt = match where_expr {
        Some(e) => quote!(__rsbatis_stmt = __rsbatis_stmt.where_(#e);),
        None => quote!(),
    };
    let order_stmts = orders
        .iter()
        .map(|(e, d)| quote!(__rsbatis_stmt = __rsbatis_stmt.order_by(#e, #d);));
    let limit_stmt = match limit {
        Some(n) => quote!(__rsbatis_stmt = __rsbatis_stmt.limit(#n);),
        None => quote!(),
    };
    let offset_stmt = match offset {
        Some(n) => quote!(__rsbatis_stmt = __rsbatis_stmt.offset(#n);),
        None => quote!(),
    };

    Ok(quote! {
        {
            let mut __rsbatis_stmt = #krate::select::<#entity>();
            #main_alias_stmt
            #distinct_stmt
            #(#join_stmts)*
            #where_stmt
            #(#order_stmts)*
            #limit_stmt
            #offset_stmt
            __rsbatis_stmt
        }
    })
}

/// 结算一个 join：`join <Entity>` 之后必须有 `on <expr>`，缺了就是写错了，
/// 不能当成"无条件连接"默默放过。
fn flush_join(
    pending: &mut Option<(syn::Ident, &'static str, Option<String>)>,
    on_toks: &mut Vec<TokenTree>,
    joins: &mut Vec<(syn::Ident, &'static str, Option<String>, Vec<TokenTree>)>,
    at: proc_macro2::Span,
) -> syn::Result<()> {
    let Some((ent, kind, alias)) = pending.take() else {
        return Ok(());
    };
    if on_toks.is_empty() {
        return Err(syn::Error::new(
            at,
            "rsbatis: `join` 之后必须写 `on <条件>`，例如 `join Order on Order.user_id == User.id`",
        ));
    }
    joins.push((ent, kind, alias, std::mem::take(on_toks)));
    Ok(())
}

/// 别名必须小写开头：`u.age` 表示别名，`User.age` 表示实体，两者靠大小写区分。
fn check_alias_name(a: &syn::Ident) -> syn::Result<()> {
    let s = a.to_string();
    let lower = s.chars().next().map(|c| c.is_lowercase()).unwrap_or(false);
    if !lower {
        return Err(syn::Error::new(
            a.span(),
            format!("rsbatis: 别名 `{s}` 必须小写开头，用来和实体类型名（`User`）区分"),
        ));
    }
    Ok(())
}

fn is_ident(t: Option<&TokenTree>, name: &str) -> bool {
    matches!(t, Some(TokenTree::Ident(i)) if i == name)
}

fn token_span(t: &TokenTree) -> proc_macro2::Span {
    t.span()
}

fn toks_to_stream(toks: Vec<TokenTree>) -> TokenStream2 {
    toks.into_iter().collect()
}

fn split_commas(toks: Vec<TokenTree>) -> Vec<Vec<TokenTree>> {
    let mut out: Vec<Vec<TokenTree>> = vec![Vec::new()];
    for t in toks {
        if let TokenTree::Punct(p) = &t {
            if p.as_char() == ',' {
                out.push(Vec::new());
                continue;
            }
        }
        out.last_mut().expect("至少有一个分组").push(t);
    }
    out
}

fn parse_u64_lit(t: Option<TokenTree>, what: &str) -> syn::Result<Option<TokenStream2>> {
    let Some(t) = t else { return Ok(None) };
    let lit: syn::LitInt = syn::parse2(TokenStream2::from(t.clone())).map_err(|_| {
        syn::Error::new(
            t.span(),
            format!("rsbatis: {what} 后面必须是整数常量（不支持变量或表达式）"),
        )
    })?;
    let v = lit
        .base10_parse::<u64>()
        .map_err(|_| syn::Error::new(lit.span(), format!("rsbatis: {what} 的值超出 u64 范围")))?;
    Ok(Some(quote!(#v)))
}

/// 给 syn 的错误补一句"在哪个段出错"的上下文，同时保留原始 span。
fn with_hint(e: syn::Error, hint: &str) -> syn::Error {
    let mut new = syn::Error::new(e.span(), format!("{hint}：{e}"));
    for inner in e.into_iter().skip(1) {
        new.combine(inner);
    }
    new
}

// ============================================================
// #[derive(FromRow)]
// ============================================================
//
// ```ignore
// #[derive(Entity, FromRow)]
// #[rsbatis(table = "users")]
// struct User { id: i64, name: String, age: Option<i32> }
//
// let users: Vec<User> = session.fetch_all(&select! { User })?;
// ```
//
// 生成的代码逐列 `get_by_name::<字段类型>(列名)`：
// - 列名与 `#[derive(Entity)]` 完全一致（同一个 `#[rsbatis(column = "...")]` 规则）；
// - 类型直接用字段原始类型（含 `Option<T>`），由 `FromValue` 在运行时校验，
//   编译期只能保证"该类型实现了 FromValue"，这是当前能达到的最强保证。

#[proc_macro_derive(FromRow, attributes(rsbatis))]
pub fn derive_from_row(input: TokenStream) -> TokenStream {
    let ast = match syn::parse::<DeriveInput>(input) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };
    match expand_from_row(&ast) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand_from_row(ast: &DeriveInput) -> syn::Result<TokenStream2> {
    let krate: TokenStream2 = CRATE.parse().expect("CRATE 是合法路径");
    let named = named_fields(ast)?;

    let mut idents: Vec<&syn::Ident> = Vec::new();
    let mut types: Vec<&Type> = Vec::new();
    let mut col_lits: Vec<LitStr> = Vec::new();

    for f in named {
        let Some(ident) = f.ident.as_ref() else {
            return Err(syn::Error::new_spanned(f, "rsbatis: 字段缺少名字"));
        };
        let col = field_column(f, ident)?;
        idents.push(ident);
        types.push(&f.ty);
        col_lits.push(LitStr::new(&col, ident.span()));
    }

    let name = &ast.ident;
    let (impl_g, ty_g, where_c) = ast.generics.split_for_impl();

    Ok(quote! {
        impl #impl_g #krate::FromRow for #name #ty_g #where_c {
            fn from_row(
                __rsbatis_row: &#krate::Row,
            ) -> ::core::result::Result<Self, #krate::DriverError> {
                ::core::result::Result::Ok(Self {
                    #(
                        #idents: __rsbatis_row.get_by_name::<#types>(#col_lits)?,
                    )*
                })
            }

            /// 联表时列名被渲染成 `表名__列名`，这里按前缀重新拼一次。
            fn from_row_prefix(
                __rsbatis_row: &#krate::Row,
                __rsbatis_prefix: &str,
            ) -> ::core::result::Result<Self, #krate::DriverError> {
                ::core::result::Result::Ok(Self {
                    #(
                        #idents: __rsbatis_row.get_by_name::<#types>(
                            &::std::format!("{__rsbatis_prefix}__{}", #col_lits),
                        )?,
                    )*
                })
            }
        }
    })
}

/// 取出具名结构体的字段，供 `derive(Entity)` / `derive(FromRow)` 共用。
fn named_fields(
    ast: &DeriveInput,
) -> syn::Result<&syn::punctuated::Punctuated<Field, syn::token::Comma>> {
    let data = match &ast.data {
        Data::Struct(s) => s,
        _ => {
            return Err(syn::Error::new_spanned(
                ast,
                "rsbatis: 该 derive 只能用于具名结构体 `struct X { .. }`",
            ))
        }
    };
    match &data.fields {
        Fields::Named(f) => Ok(&f.named),
        _ => Err(syn::Error::new_spanned(
            &data.fields,
            "rsbatis: 需要具名字段，元组结构体不支持",
        )),
    }
}

// ============================ update! / delete! ============================

/// `update!`：带字段清单的批量更新。
///
/// ```ignore
/// // 把 age >= 18 的人的 status 改成 1
/// update!(User { status: 1 } where User.age >= 18)
/// ```
///
/// 字段值走 `UpdateStmt::set` 的 `impl IntoValue<T>` 约束，所以类型仍然是 rustc 校验的：
/// 给 `age` 传字符串会在编译期报错，不会拖到运行时。
///
/// 为什么是"结构体字面量"形态而不是 `.set_age(25)`：Rust 的方法名必须在类型定义时就存在，
/// 宏无法为一个泛型 `UpdateStmt` 动态生成 `set_<任意字段>` 方法。用字面量形态，
/// 字段名在宏里直接变成 `<E>::col_<field>()`，既准确又能让编译器校验类型。
#[proc_macro]
pub fn update(input: TokenStream) -> TokenStream {
    // 先转 proc_macro2：展开函数的入参类型统一成 TokenStream2 才能在 `#[cfg(test)]` 里直接调，
    // 错误路径（空字段、拼写、多余 token）才可能被单元测试覆盖 —— proc_macro::TokenStream
    // 在测试里造不出来。
    match expand_update(input.into()) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// `delete!`：带条件的批量删除。
///
/// ```ignore
/// delete!(User where User.age < 0)
/// delete!(User)                       // 全表删除：合法但危险，自己确认
/// ```
#[proc_macro]
pub fn delete(input: TokenStream) -> TokenStream {
    match expand_delete(input.into()) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// `update!` 的解析结果。
struct UpdateInput {
    entity: syn::Ident,
    sets: Vec<(syn::Ident, syn::Expr)>,
    filter: Option<syn::Expr>,
}

impl syn::parse::Parse for UpdateInput {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let entity: syn::Ident = input.parse().map_err(|_| {
            syn::Error::new(
                input.span(),
                "rsbatis: update! 的第一段必须是实体类型名，例如 `update!(User { age: 25 } where …)`",
            )
        })?;
        if !entity.to_string().starts_with(char::is_uppercase) {
            return Err(syn::Error::new_spanned(
                &entity,
                "rsbatis: 实体名应以大写字母开头（这是类型，不是变量）",
            ));
        }

        let mut sets: Vec<(syn::Ident, syn::Expr)> = Vec::new();
        if input.peek(syn::token::Brace) {
            let content;
            syn::braced!(content in input);
            while !content.is_empty() {
                let field: syn::Ident = content.parse().map_err(|_| {
                    syn::Error::new(
                        content.span(),
                        "rsbatis: 这里需要一个字段名，例如 `age: 25`",
                    )
                })?;
                content.parse::<syn::Token![:]>().map_err(|_| {
                    syn::Error::new(
                        field.span(),
                        "rsbatis: 字段名后面要跟冒号和新值，例如 `age: 25`",
                    )
                })?;
                let value: syn::Expr = content.parse()?;
                sets.push((field, value));
                if content.peek(syn::Token![,]) {
                    content.parse::<syn::Token![,]>()?;
                } else {
                    break;
                }
            }
        }
        if sets.is_empty() {
            return Err(syn::Error::new_spanned(
                &entity,
                "rsbatis: update! 至少要给一个字段，例如 `update!(User { age: 25 } …)`",
            ));
        }

        let filter = if input.peek(syn::Token![where]) {
            input.parse::<syn::Token![where]>()?;
            Some(input.parse::<syn::Expr>()?)
        } else {
            None
        };
        if !input.is_empty() {
            return Err(syn::Error::new(
                input.span(),
                "rsbatis: update! 里 `where` 之后只能是一个条件表达式",
            ));
        }
        Ok(Self {
            entity,
            sets,
            filter,
        })
    }
}

fn expand_update(input: TokenStream2) -> syn::Result<TokenStream2> {
    let UpdateInput {
        entity,
        sets,
        filter,
    } = syn::parse2::<UpdateInput>(input)?;

    let core: TokenStream2 = CORE.parse().expect("CORE 是合法路径");
    let mut body = quote! {
        let mut __stmt = #core::UpdateStmt::new(<#entity as #core::Entity>::TABLE);
    };
    for (field, value) in sets {
        let col = syn::Ident::new(&format!("col_{}", field), field.span());
        body.extend(quote! {
            __stmt = __stmt.set(<#entity>::#col(), #value);
        });
    }
    if let Some(f) = filter {
        let node = rewrite(&f, &core, &Aliases::new())?;
        let expr = node.into_expr(&core);
        body.extend(quote! { __stmt = __stmt.where_(#expr); });
    }
    Ok(quote! {{ #body __stmt }})
}

/// `delete!` 的解析结果。
struct DeleteInput {
    entity: syn::Ident,
    filter: Option<syn::Expr>,
}

impl syn::parse::Parse for DeleteInput {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let entity: syn::Ident = input.parse().map_err(|_| {
            syn::Error::new(
                input.span(),
                "rsbatis: delete! 的第一段必须是实体类型名，例如 `delete!(User where User.age < 0)`",
            )
        })?;
        if !entity.to_string().starts_with(char::is_uppercase) {
            return Err(syn::Error::new_spanned(
                &entity,
                "rsbatis: 实体名应以大写字母开头（这是类型，不是变量）",
            ));
        }
        let filter = if input.peek(syn::Token![where]) {
            input.parse::<syn::Token![where]>()?;
            Some(input.parse::<syn::Expr>()?)
        } else {
            None
        };
        if !input.is_empty() {
            return Err(syn::Error::new(
                input.span(),
                "rsbatis: delete! 里 `where` 之后只能是一个条件表达式",
            ));
        }
        Ok(Self { entity, filter })
    }
}

fn expand_delete(input: TokenStream2) -> syn::Result<TokenStream2> {
    let DeleteInput { entity, filter } = syn::parse2::<DeleteInput>(input)?;
    let core: TokenStream2 = CORE.parse().expect("CORE 是合法路径");
    let mut body = quote! {
        let mut __stmt = #core::DeleteStmt::new(<#entity as #core::Entity>::TABLE);
    };
    if let Some(f) = filter {
        let node = rewrite(&f, &core, &Aliases::new())?;
        let expr = node.into_expr(&core);
        body.extend(quote! { __stmt = __stmt.where_(#expr); });
    }
    Ok(quote! {{ #body __stmt }})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upd(tokens: &str) -> syn::Result<String> {
        let ts: TokenStream2 = tokens.parse().expect("测试 token 合法");
        expand_update(ts).map(|t| t.to_string())
    }

    fn del(tokens: &str) -> syn::Result<String> {
        let ts: TokenStream2 = tokens.parse().expect("测试 token 合法");
        expand_delete(ts).map(|t| t.to_string())
    }

    fn err_msg<T: std::fmt::Debug>(r: syn::Result<T>) -> String {
        r.expect_err("应当报错").to_string()
    }

    #[test]
    fn update_expands_one_set_per_field() {
        let out = upd("User { age: 25 } where User.age >= 18").unwrap();
        assert!(out.contains("UpdateStmt :: new"), "got: {out}");
        assert!(out.contains("col_age"), "got: {out}");
        assert!(out.contains("where_"), "got: {out}");
    }

    #[test]
    fn update_expands_multiple_fields_in_order() {
        let out = upd(r#"User { name: "x", age: 1 }"#).unwrap();
        let (n, a) = (out.find("col_name").unwrap(), out.find("col_age").unwrap());
        assert!(n < a, "字段顺序应保持书写顺序，got: {out}");
    }

    #[test]
    fn update_without_where_has_no_filter() {
        assert!(!upd("User { age: 1 }").unwrap().contains("where_"));
    }

    #[test]
    fn update_rejects_empty_field_list() {
        let m = err_msg(upd("User {}"));
        assert!(m.contains("至少要给一个字段"), "got: {m}");
    }

    #[test]
    fn update_rejects_lowercase_entity() {
        let m = err_msg(upd("user { age: 1 }"));
        assert!(m.contains("大写字母"), "got: {m}");
    }

    #[test]
    fn update_rejects_missing_colon() {
        let m = err_msg(upd("User { age 1 }"));
        assert!(m.contains("冒号"), "got: {m}");
    }

    #[test]
    fn update_rejects_trailing_tokens() {
        let m = err_msg(upd("User { age: 1 } where User.id == 1 limit 3"));
        assert!(m.contains("只能是一个条件表达式"), "got: {m}");
    }

    #[test]
    fn update_rejects_non_ident_first_token() {
        let m = err_msg(upd("42 { age: 1 }"));
        assert!(m.contains("实体类型名"), "got: {m}");
    }

    #[test]
    fn delete_expands_with_where() {
        let out = del("User where User.age < 0").unwrap();
        assert!(out.contains("DeleteStmt :: new"), "got: {out}");
        assert!(out.contains("where_"), "got: {out}");
    }

    #[test]
    fn delete_without_where_is_allowed() {
        assert!(!del("User").unwrap().contains("where_"));
    }

    #[test]
    fn delete_rejects_lowercase_entity() {
        assert!(err_msg(del("user")).contains("大写字母"));
    }

    #[test]
    fn delete_rejects_trailing_tokens() {
        assert!(err_msg(del("User where User.id == 1 extra")).contains("只能是一个条件表达式"));
    }

    #[test]
    fn delete_rejects_non_ident_first_token() {
        assert!(err_msg(del("3 where User.id == 1")).contains("实体类型名"));
    }

    #[test]
    fn where_segment_still_rewrites_columns() {
        // 条件里的 `User.age` 必须被重写成实体列，不能原样留下（Rust 里那根本不是合法表达式）
        let out = upd("User { age: 1 } where User.age >= 18").unwrap();
        assert!(out.contains("col_age"), "got: {out}");
    }
}
