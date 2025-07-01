use proc_macro::TokenStream;
use quote::{format_ident, quote};
use sha3::{Digest, Sha3_256};
use syn::{ItemStruct, parse_macro_input};

#[proc_macro_attribute]
pub fn handle(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item: ItemStruct = parse_macro_input!(item);
    let attr = attr.to_string().replace("\n", " ");
    let mut hasher = Sha3_256::new();

    let event_handlers = attr
        .split(", ")
        .map(|h| {
            let h = h.trim();
            hasher.update(h);
            let name = format_ident!("{}", h);

            quote! {
              if let Ok(Some(detail)) = event.to_detail() {
                  self.#name(executor, detail).await?;
                  return Ok(());
              }
            }
        })
        .collect::<proc_macro2::TokenStream>();

    let ident = item.ident.clone();

    quote! {
        #item

        #[async_trait::async_trait]
        impl SubscribeHandler for #ident {
            async fn handle<E: Executor>(&self, executor: &E, event: &Event) -> anyhow::Result<()> {
                #event_handlers

                Ok(())
            }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn aggregate(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item: ItemStruct = parse_macro_input!(item);
    let attr = attr.to_string().replace("\n", " ");
    let mut hasher = Sha3_256::new();

    let event_handlers = attr
        .split(", ")
        .map(|h| {
            let h = h.trim();
            hasher.update(h);
            let name = format_ident!("{}", h);

            quote! {
              if let Ok(Some(detail)) = event.to_detail() {
                  self.#name(detail).await?;
                  return Ok(());
              }
            }
        })
        .collect::<proc_macro2::TokenStream>();

    let ident = item.ident.clone();
    let revision = format!("{:x}", hasher.finalize());
    let name = item.ident.to_string();

    quote! {
        #item

        #[async_trait::async_trait]
        impl Aggregator for #ident {
            async fn aggregate(&mut self, event: Event) -> anyhow::Result<()> {
                #event_handlers

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
