use proc_macro::TokenStream;
use quote::{format_ident, quote};
use sha3::{Digest, Sha3_256};
use syn::{ItemStruct, parse_macro_input};

#[proc_macro_attribute]
pub fn aggregate(attr: TokenStream, item: TokenStream) -> TokenStream {
    let ItemStruct { ident, fields, .. } = parse_macro_input!(item);
    let attr = attr.to_string().replace("\n", " ");
    let mut hasher = Sha3_256::new();

    let event_handlers = attr
        .split(", ")
        .map(|h| {
            hasher.update(h);
            let name = format_ident!("{}", h);

            quote! {
              if let (Some(data), Some(metadata)) = (event.to_data(), event.to_metadata()) {
                  self.#name(event, data, metadata);
                  return;
              }
            }
        })
        .collect::<proc_macro2::TokenStream>();

    let revision = format!("{:x}", hasher.finalize());

    quote! {
        struct #ident
        #fields

        impl Aggregator for #ident {
            fn aggregate(&mut self, event: &'_ Event) {
                #event_handlers
            }

            fn revision() -> &'static str {
                #revision
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
