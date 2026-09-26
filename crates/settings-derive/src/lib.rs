//! `#[derive(Settings)]` on nori-settings' `StoredPrefs`: every field says, in one `#[setting(...)]` line,
//! how it is stored, changed and read by name, and what a client offers for it, and this makes the
//! struct's `Default` and the table the settings are walked by (`ROWS`, of `codec::Row`s):
//!
//! ```text
//! #[setting("storedKey", CODEC, default = EXPR [, name = "name" | hidden] [, show = K::...] [, effect = BITS] [, lookups])]
//! ```
//!
//! - `"storedKey"`: the key it is stored under; also the name it is changed and read by, unless `name`
//!   says another, or `hidden` says it has none (it is changed only as part of the whole record);
//! - `CODEC`: how its value is stored, parsed and shown (a `codec::Codec` for the field's type);
//! - `default`: its value out of the box;
//! - `show`: what a client's settings screen offers for it (`codec::K`); none for a setting not offered;
//! - `effect`: what a change of it asks of the player (`settings_store`'s bits);
//! - `lookups`: a switch over something looked up online: it reads off while the lookups are off, and
//!   switching it on switches them on.
//!
//! The struct itself stays written out, as uniffi's bindings generator reads it from the source.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, Data, DeriveInput, Expr, Fields, Ident, LitStr, Token};

struct Setting {
    key: LitStr,
    codec: Expr,
    default: Option<Expr>,
    name: Option<LitStr>,
    hidden: bool,
    show: Option<Expr>,
    effect: Option<Expr>,
    lookups: bool,
}

impl Parse for Setting {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: LitStr = input.parse()?;
        input.parse::<Token![,]>()?;
        let codec: Expr = input.parse()?;
        let mut s = Setting { key, codec, default: None, name: None, hidden: false, show: None, effect: None, lookups: false };
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let word: Ident = input.parse()?;
            match word.to_string().as_str() {
                "hidden" => s.hidden = true,
                "lookups" => s.lookups = true,
                "default" | "name" | "show" | "effect" => {
                    input.parse::<Token![=]>()?;
                    match word.to_string().as_str() {
                        "default" => s.default = Some(input.parse()?),
                        "name" => s.name = Some(input.parse()?),
                        "show" => s.show = Some(input.parse()?),
                        _ => s.effect = Some(input.parse()?),
                    }
                }
                other => return Err(syn::Error::new(word.span(), format!("unknown setting part `{other}`"))),
            }
        }
        Ok(s)
    }
}

#[proc_macro_derive(Settings, attributes(setting))]
pub fn derive_settings(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(t) => t.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// `#[derive(Choice)]` on an enum setting of plain variants: `codec::Choice` with every variant in order and
/// each one's name as it is stored and changed by, SCREAMING_SNAKE_CASE as the Kotlin bindings name them
/// (`PlayNext` is "PLAY_NEXT").
#[proc_macro_derive(Choice)]
pub fn derive_choice(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ty = &input.ident;
    let Data::Enum(data) = &input.data else { return syn::Error::new_spanned(ty, "a choice is an enum").to_compile_error().into() };
    let variants: Vec<&Ident> = data.variants.iter().map(|v| &v.ident).collect();
    let names = variants.iter().map(|v| {
        let mut s = String::new();
        for (i, c) in v.to_string().chars().enumerate() {
            if c.is_uppercase() && i > 0 {
                s.push('_');
            }
            s.push(c.to_ascii_uppercase());
        }
        s
    });
    quote! {
        impl crate::codec::Choice for #ty {
            const ALL: &'static [Self] = &[#(#ty::#variants,)*];
            const NAMES: &'static [&'static str] = &[#(#names,)*];
        }
    }
    .into()
}

fn expand(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let ty = &input.ident;
    let Data::Struct(data) = &input.data else { return Err(syn::Error::new_spanned(ty, "settings are a struct")) };
    let Fields::Named(fields) = &data.fields else { return Err(syn::Error::new_spanned(ty, "settings have named fields")) };
    let mut defaults = Vec::new();
    let mut rows = Vec::new();
    for f in &fields.named {
        let field = f.ident.as_ref().unwrap();
        let fty = &f.ty;
        let attr = f
            .attrs
            .iter()
            .find(|a| a.path().is_ident("setting"))
            .ok_or_else(|| syn::Error::new_spanned(field, "every setting needs its #[setting(...)] line"))?;
        let s: Setting = attr.parse_args()?;
        let default = s.default.as_ref().ok_or_else(|| syn::Error::new_spanned(attr, "a setting needs its `default`"))?;
        defaults.push(quote! { #field: #default });
        let Setting { key, codec, show, effect, lookups, .. } = &s;
        let name = match (&s.name, s.hidden) {
            (_, true) => quote! { None },
            (Some(n), false) => quote! { Some(#n) },
            (None, false) => quote! { Some(#key) },
        };
        let show = show.as_ref().map_or_else(|| quote! { None }, |e| quote! { Some(#e) });
        let effect = effect.as_ref().map_or_else(|| quote! { 0 }, |e| quote! { #e });
        rows.push(quote! {
            crate::codec::Row {
                key: #key,
                name: #name,
                spec: #show,
                effect: #effect,
                lookups: #lookups,
                load: |p, r| p.#field = crate::codec::Codec::<#fty>::load(&#codec, r, #key, p.#field.clone()),
                save: |p, out| crate::codec::Codec::<#fty>::save(&#codec, &p.#field, #key, out),
                set: |p, v| {
                    p.#field = crate::codec::Codec::<#fty>::set(&#codec, v, &p.#field)?;
                    Some(())
                },
                show: |p| crate::codec::Codec::<#fty>::show(&#codec, &p.#field),
                changed: |a, b| a.#field != b.#field,
            }
        });
    }
    Ok(quote! {
        impl Default for #ty {
            fn default() -> Self {
                #ty { #(#defaults,)* }
            }
        }

        /// Every setting, in the order the struct declares them.
        pub(crate) static ROWS: &[crate::codec::Row] = &[#(#rows,)*];
    })
}
