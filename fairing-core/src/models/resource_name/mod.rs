//! Resource name traits.
//!
//! # Examples:
//!
//! ```
//! use fairing_core::models::resource_name::{ResourceName, ParentedResourceName, ResourceNameInner, validators};
//!
//! /// Resource name without a parent: `teams/test-team`.
//! fairing_core::impl_resource_name! {
//!     pub struct TeamName<'n>;
//! }
//!
//! impl<'n> ResourceName<'n> for TeamName<'n> {
//!     const COLLECTION: &'static str = "teams";
//!
//!     type Validator = validators::UnicodeIdentifierValidator;
//! }
//!
//! /// Resource name with a parent: `teams/test-team/sites/test-site`.
//! fairing_core::impl_resource_name! {
//!     pub struct SiteName<'n>;
//! }
//!
//! impl<'n> ParentedResourceName<'n> for SiteName<'n> {
//!     const COLLECTION: &'static str = "sites";
//!
//!     type Validator = validators::UnicodeIdentifierValidator;
//!
//!     type Parent = TeamName<'static>;
//! }
//! ```
use anyhow::{anyhow, Result};
use std::{borrow::Cow, num::NonZeroUsize};

use validators::ResourceIDValidator;

pub mod validators;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceNameInner<'n> {
    pub name: Cow<'n, str>,
    pub parent_len: Option<NonZeroUsize>,
    pub resource_len: Option<NonZeroUsize>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MatchedResourceName {
    pub parent_len: usize,
    pub resource_len: usize,
    pub len: usize,
}

pub trait ResourceNameConstructor<'n>: Clone {
    fn from_inner(inner: ResourceNameInner<'n>) -> Self;

    fn inner(&'_ self) -> &'_ ResourceNameInner<'_>;
}

/// A resource name: `users/test@example.com`. For resource names with parents see
/// `ParentedResourceName`.
///
/// Follows the guidelines here: <https://cloud.google.com/apis/design/resource_names>
pub trait ResourceName<'n>: ResourceNameConstructor<'n> {
    /// Name of the collection.
    const COLLECTION: &'static str;

    /// The number of parents above this part of the name.
    const PARENT_DEPTH: usize = 0;

    /// Validator for the resource ID.
    type Validator: validators::ResourceIDValidator;

    /// Gets the full resource name.
    #[inline]
    fn name(&self) -> &str {
        &self.inner().name
    }

    /// Gets the resource ID.
    #[inline]
    fn resource(&self) -> &str {
        let inner = self.inner();
        let resource_len = match inner.resource_len {
            Some(resource_len) => resource_len.get(),
            None => Self::match_len(&inner.name).unwrap().resource_len,
        };

        &inner.name[inner.name.len() - resource_len..]
    }

    /// Parse the resource name. Returns `None` if the resource name is invalid. Also validates any
    /// parents.
    fn parse(name: impl Into<Cow<'n, str>>) -> Result<Self> {
        use unicode_normalization::{is_nfc_quick, IsNormalized, UnicodeNormalization};

        let name = name.into();
        let name = match is_nfc_quick(name.chars()) {
            IsNormalized::Yes => name,
            IsNormalized::No | IsNormalized::Maybe => Cow::Owned(name.nfc().collect::<String>()),
        };

        let matched =
            Self::match_len(&name).ok_or_else(|| anyhow!("name does not match pattern"))?;

        if matched.len != name.len() {
            Err(anyhow!("name is of wrong type"))
        } else {
            let inner = ResourceNameInner {
                name,
                parent_len: NonZeroUsize::new(matched.parent_len),
                resource_len: Some(NonZeroUsize::new(matched.resource_len).unwrap()),
            };

            Ok(Self::from_inner(inner))
        }
    }

    /// Gets the length of the matched resource name and its parent.
    ///
    /// For example, if our collection is called "teams", this function would match only the first
    /// part of the input.
    ///
    /// ```
    /// # use fairing_core::models::resource_name::*;
    /// # fairing_core::impl_resource_name! {
    /// #     pub struct TeamName<'n>;
    /// # }
    /// # impl<'n> ResourceName<'n> for TeamName<'n> {
    /// #     const COLLECTION: &'static str = "teams";
    /// #     type Validator = validators::AnyValidator;
    /// # }
    /// assert_eq!(
    ///     TeamName::match_len("teams/test-team/sites/test-site"),
    ///     Some(MatchedResourceName { parent_len: 0, resource_len: 9, len: 15 }),
    /// );
    /// ```
    fn match_len(name: &str) -> Option<MatchedResourceName> {
        let without_collection = name.strip_prefix(Self::COLLECTION)?.strip_prefix('/')?;

        let resource = Self::Validator::validate(without_collection)?;

        let len = (name.len() - without_collection.len()) + resource.len();

        Some(MatchedResourceName {
            parent_len: 0,
            resource_len: resource.len(),
            len,
        })
    }
}

/// Resource name with a parent: `teams/fairing/sites/test-site`.
pub trait ParentedResourceName<'n>: ResourceNameConstructor<'n> {
    const COLLECTION: &'static str;

    type Validator: validators::ResourceIDValidator;

    type Parent: ResourceName<'static>;

    fn parent(&self) -> Self::Parent {
        let inner = self.inner();
        let parent_len = match inner.parent_len {
            Some(parent_len) => parent_len.get(),
            None => {
                // No need to parse everything, just use PARENT_DEPTH to decide how many bytes
                // we need to grab.
                inner
                    .name
                    .split('/')
                    .take(Self::PARENT_DEPTH * 2)
                    .map(|s| s.len())
                    .sum::<usize>()
                    + Self::PARENT_DEPTH * 2
                    - 1
            }
        };

        let parent = &inner.name[..parent_len];
        let resource_len = parent
            .rsplitn(2, '/')
            .next()
            .and_then(|s| NonZeroUsize::new(s.len()));

        let inner = ResourceNameInner {
            name: Cow::Owned(parent.to_owned()),
            parent_len: None,
            resource_len,
        };

        Self::Parent::from_inner(inner)
    }
}

impl<'n, T> ResourceName<'n> for T
where
    T: Clone + ParentedResourceName<'n>,
{
    const COLLECTION: &'static str = <Self as ParentedResourceName<'n>>::COLLECTION;

    const PARENT_DEPTH: usize = <Self as ParentedResourceName<'n>>::Parent::PARENT_DEPTH + 1;

    type Validator = <Self as ParentedResourceName<'n>>::Validator;

    fn match_len(name: &str) -> Option<MatchedResourceName> {
        let MatchedResourceName {
            len: parent_len, ..
        } = <Self as ParentedResourceName<'n>>::Parent::match_len(name)?;

        let name = &name[parent_len..];
        let without_collection = name
            .strip_prefix('/')?
            .strip_prefix(Self::COLLECTION)?
            .strip_prefix('/')?;

        let resource = Self::Validator::validate(without_collection)?;

        let len = parent_len + (name.len() - without_collection.len()) + resource.len();

        Some(MatchedResourceName {
            parent_len,
            resource_len: resource.len(),
            len,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;

    crate::impl_resource_name! {
        pub struct TeamName<'n>;
        pub struct SiteName<'n>;
        pub struct SiteSourceName<'n>;
    }

    impl<'n> ResourceName<'n> for TeamName<'n> {
        const COLLECTION: &'static str = "teams";

        type Validator = validators::AnyValidator;
    }

    impl<'n> ParentedResourceName<'n> for SiteName<'n> {
        const COLLECTION: &'static str = "sites";

        type Validator = validators::AnyValidator;

        type Parent = TeamName<'static>;
    }

    impl<'n> ParentedResourceName<'n> for SiteSourceName<'n> {
        const COLLECTION: &'static str = "sources";

        type Validator = validators::AnyValidator;

        type Parent = SiteName<'static>;
    }

    #[test]
    fn test_simple_match_len() {
        assert_eq!(
            TeamName::match_len("teams/test-team/sites/test-site"),
            Some(MatchedResourceName {
                parent_len: 0,
                resource_len: 9,
                len: 15
            }),
        );
    }

    #[test]
    fn test_match_len_with_parent() {
        assert_eq!(
            SiteName::match_len("teams/test-team/sites/test-site"),
            Some(MatchedResourceName {
                parent_len: 15,
                resource_len: 9,
                len: 31
            }),
        );
    }

    #[test]
    fn test_parse() {
        assert_eq!(
            TeamName::parse("teams/test-team").ok(),
            Some(TeamName(ResourceNameInner {
                name: Cow::Borrowed("teams/test-team"),
                parent_len: None,
                resource_len: NonZeroUsize::new(9),
            })),
        );

        assert_eq!(
            TeamName::parse("teams/test-team/sites/test-site").ok(),
            None
        );

        assert_eq!(
            SiteName::parse("teams/test-team/sites/test-site").ok(),
            Some(SiteName(ResourceNameInner {
                name: Cow::Borrowed("teams/test-team/sites/test-site"),
                parent_len: NonZeroUsize::new(15),
                resource_len: NonZeroUsize::new(9),
            })),
        );
    }

    #[test]
    fn test_parent() {
        let site_source_name =
            SiteSourceName::parse("teams/test-team/sites/test-site/sources/test-source").unwrap();

        assert_eq!(
            site_source_name.parent(),
            SiteName(ResourceNameInner {
                name: Cow::Borrowed("teams/test-team/sites/test-site"),
                parent_len: None,
                resource_len: NonZeroUsize::new(9),
            }),
        );

        assert_eq!(
            site_source_name.parent().parent(),
            TeamName(ResourceNameInner {
                name: Cow::Borrowed("teams/test-team"),
                parent_len: None,
                resource_len: NonZeroUsize::new(9),
            }),
        );
    }

    #[test]
    fn test_resource() {
        assert_eq!(
            SiteName::parse("teams/test-team/sites/test-site")
                .unwrap()
                .resource(),
            "test-site",
        );
    }
}
