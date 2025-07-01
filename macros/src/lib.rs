use proc_macro::TokenStream;
use quote::quote;
use sha3::{Digest, Sha3_256};
use std::ops::Deref;
use syn::{ItemImpl, ItemStruct, parse_macro_input};

#[proc_macro_attribute]
pub fn handle(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item: ItemImpl = parse_macro_input!(item);

    let syn::Type::Path(item_path) = item.self_ty.deref() else {
        return syn::Error::new_spanned(item, "Unable to find name of impl struct")
            .into_compile_error()
            .into();
    };

    let Some(ident) = item_path.path.get_ident() else {
        return syn::Error::new_spanned(item, "Unable to get ident of impl struct")
            .into_compile_error()
            .into();
    };

    let handler_fns = item
        .items
        .iter()
        .filter_map(|item| {
            let syn::ImplItem::Fn(iten_fn) = item else {
                return None;
            };

            let ident = iten_fn.sig.ident.clone();

            Some(quote! {
                if let Ok(Some(context)) = context.to_context(){
                    self.#ident(context).await?;
                    return Ok(());
                }
            })
        })
        .collect::<proc_macro2::TokenStream>();

    quote! {
        #item

        #[async_trait::async_trait]
        impl<E: liteventd::Executor> SubscribeHandler<E> for #ident {
            async fn handle(&self, context: &liteventd::ContextBase<'_, E>) -> anyhow::Result<()> {
                #handler_fns

                Ok(())
            }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn aggregate(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item: ItemImpl = parse_macro_input!(item);
    let mut hasher = Sha3_256::new();

    let syn::Type::Path(item_path) = item.self_ty.deref() else {
        return syn::Error::new_spanned(item, "Unable to find name of impl struct")
            .into_compile_error()
            .into();
    };

    let Some(ident) = item_path.path.get_ident() else {
        return syn::Error::new_spanned(item, "Unable to get ident of impl struct")
            .into_compile_error()
            .into();
    };

    let handler_fns = item
        .items
        .iter()
        .filter_map(|item| {
            let syn::ImplItem::Fn(iten_fn) = item else {
                return None;
            };

            hasher.update(iten_fn.sig.ident.to_string());
            let ident = iten_fn.sig.ident.clone();

            Some(quote! {
                if let Ok(Some(data)) = event.to_data(){
                    self.#ident(data).await?;
                    return Ok(());
                }
            })
        })
        .collect::<proc_macro2::TokenStream>();

    let revision = format!("{:x}", hasher.finalize());
    let name = ident.to_string();

    quote! {
        #item

        #[async_trait::async_trait]
        impl Aggregator for #ident {
            async fn aggregate(&mut self, event: Event) -> anyhow::Result<()> {
                #handler_fns

                Ok(())
            }

            fn revision() -> &'static str {
                #revision
            }

            fn name() -> &'static str {
                #name
            }
        }
    }
    .into()
}

#[proc_macro_derive(AggregatorEvent)]
pub fn derive_aggregator_event(input: TokenStream) -> TokenStream {
    let ItemStruct { ident, .. } = parse_macro_input!(input);
    let name = ident.to_string();

    quote! {
        impl AggregatorEvent for #ident {
            fn name() -> &'static str {
                #name
            }
        }
    }
    .into()
}
