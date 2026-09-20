pub mod address_book;
pub mod company_identity;

pub use address_book::{AddressBookView, load as load_address_book};
pub use company_identity::{CompanyIdentityView, VatCheckRecord, load as load_company_identity};
