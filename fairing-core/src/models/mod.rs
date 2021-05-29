//! Domain models

pub use sites::*;
pub use teams::*;
pub use users::*;

pub mod prelude;
pub mod resource_name;
mod sites;
mod teams;
mod users;

#[macro_export]
macro_rules! impl_resource_name {
    { $( pub struct $type:ident<$lifetime:lifetime>; )* } => {
        $(
            #[derive(Clone, Debug, PartialEq)]
            pub struct $type<$lifetime>($crate::models::resource_name::ResourceNameInner<$lifetime>);

            impl<'n> $crate::models::resource_name::ResourceNameConstructor<'n> for $type<'n> {
                fn from_inner(inner: ResourceNameInner<'n>) -> Self {
                    Self(inner)
                }

                fn inner(&'_ self) -> &'_ ResourceNameInner<'_> {
                    &self.0
                }
            }

            impl<'r, DB: sqlx::Database> sqlx::decode::Decode<'r, DB> for $type<'static>
            where
                String: sqlx::decode::Decode<'r, DB>,
            {
                fn decode(
                    value: <DB as sqlx::database::HasValueRef<'r>>::ValueRef,
                ) -> Result<$type<'static>, Box<dyn std::error::Error + 'static + Send + Sync>> {
                    let value = <String as sqlx::decode::Decode<DB>>::decode(value)?;

                    Ok($type(ResourceNameInner {
                        name: ::std::borrow::Cow::Owned(value),
                        parent_len: None,
                        resource_len: None,
                    }))
                }
            }

            impl ::sqlx::types::Type<::sqlx::Postgres> for $type<'static> {
                fn type_info() -> ::sqlx::postgres::PgTypeInfo {
                    <String as ::sqlx::types::Type<::sqlx::Postgres>>::type_info()
                }
            }
        )*
    };
}
