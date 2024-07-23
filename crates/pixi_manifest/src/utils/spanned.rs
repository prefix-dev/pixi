//! Taken from: <https://docs.rs/serde_spanned/latest/serde_spanned/struct.Spanned.html>
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

// Currently serde itself doesn't have a spanned type, so we map our `Spanned`
// to a special value in the serde data model. Namely one with these special
// fields/struct names.
//
// In general, supported deserializers should catch this and not literally emit
// these strings but rather emit `Spanned` as they're intended.
#[doc(hidden)]
pub const NAME: &str = "$__serde_spanned_private_Spanned";
#[doc(hidden)]
pub const START_FIELD: &str = "$__serde_spanned_private_start";
#[doc(hidden)]
pub const END_FIELD: &str = "$__serde_spanned_private_end";
#[doc(hidden)]
pub const VALUE_FIELD: &str = "$__serde_spanned_private_value";

/// A spanned value, indicating the range at which it is defined in the source.
#[derive(Clone, Debug)]
pub struct PixiSpanned<T> {
    /// Byte range
    pub span: Option<std::ops::Range<usize>>,
    /// The spanned value.
    pub value: T,
}

impl<T: Default> Default for PixiSpanned<T> {
    fn default() -> Self {
        Self {
            span: None,
            value: T::default(),
        }
    }
}

impl<T> From<T> for PixiSpanned<T> {
    fn from(value: T) -> Self {
        Self { span: None, value }
    }
}

impl<T> PixiSpanned<T> {
    /// Byte range
    pub fn span(&self) -> Option<std::ops::Range<usize>> {
        self.span.clone()
    }

    /// Consumes the spanned value and returns the contained value.
    pub fn into_inner(self) -> T {
        self.value
    }

    /// Returns a reference to the contained value.
    pub fn get_ref(&self) -> &T {
        &self.value
    }

    /// Returns a mutable reference to the contained value.
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl std::borrow::Borrow<str> for PixiSpanned<String> {
    fn borrow(&self) -> &str {
        self.get_ref()
    }
}

impl<T> AsRef<T> for PixiSpanned<T> {
    fn as_ref(&self) -> &T {
        self.get_ref()
    }
}

impl<T> AsMut<T> for PixiSpanned<T> {
    fn as_mut(&mut self) -> &mut T {
        self.get_mut()
    }
}

impl<T: PartialEq> PartialEq for PixiSpanned<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value.eq(&other.value)
    }
}

impl<T: Eq> Eq for PixiSpanned<T> {}

impl<T: Hash> Hash for PixiSpanned<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T: PartialOrd> PartialOrd for PixiSpanned<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.value.partial_cmp(&other.value)
    }
}

impl<T: Ord> Ord for PixiSpanned<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value.cmp(&other.value)
    }
}

impl<'de, T> serde::de::Deserialize<'de> for PixiSpanned<T>
where
    T: serde::de::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<PixiSpanned<T>, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        struct SpannedVisitor<T>(::std::marker::PhantomData<T>);

        impl<'de, T> serde::de::Visitor<'de> for SpannedVisitor<T>
        where
            T: serde::de::Deserialize<'de>,
        {
            type Value = PixiSpanned<T>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a spanned value")
            }

            fn visit_map<V>(self, mut visitor: V) -> Result<PixiSpanned<T>, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                if visitor.next_key()? != Some(START_FIELD) {
                    return Err(serde::de::Error::custom("spanned start key not found"));
                }
                let start: usize = visitor.next_value()?;

                if visitor.next_key()? != Some(END_FIELD) {
                    return Err(serde::de::Error::custom("spanned end key not found"));
                }
                let end: usize = visitor.next_value()?;

                if visitor.next_key()? != Some(VALUE_FIELD) {
                    return Err(serde::de::Error::custom("spanned value key not found"));
                }
                let value: T = visitor.next_value()?;

                Ok(PixiSpanned {
                    span: Some(start..end),
                    value,
                })
            }
        }

        let visitor = SpannedVisitor(::std::marker::PhantomData);

        static FIELDS: [&str; 3] = [START_FIELD, END_FIELD, VALUE_FIELD];
        deserializer.deserialize_struct(NAME, &FIELDS, visitor)
    }
}

impl<T: serde::ser::Serialize> serde::ser::Serialize for PixiSpanned<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        self.value.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::spanned::PixiSpanned;
    use serde::Deserialize;

    #[test]
    pub fn test_spanned() {
        #[derive(Deserialize)]
        struct Value {
            s: PixiSpanned<String>,
        }

        let t = "s = \"value\"\n";

        let u: Value = toml_edit::de::from_str(t).unwrap();

        assert_eq!(u.s.span().unwrap(), 4..11);
        assert_eq!(u.s.get_ref(), "value");
        assert_eq!(u.s.into_inner(), String::from("value"));
    }
}
