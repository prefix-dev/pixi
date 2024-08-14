use serde_with::serde_derive::Deserialize;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub(crate) struct EnvironmentIdx(pub(crate) usize);

#[derive(Deserialize, Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) struct EnvironmentName(pub(crate) String);
