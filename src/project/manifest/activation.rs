use serde::Deserialize;

#[derive(Default, Clone, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Activation {
    pub scripts: Option<Vec<String>>,
}

impl Activation {
    /// Constructs a new activation where the scripts are the concatenation of the scripts in
    /// `self`.
    pub fn append(&self, other: &Activation) -> Activation {
        let scripts = match (&self.scripts, &other.scripts) {
            (Some(a), Some(b)) => Some(a.iter().chain(b.iter()).cloned().collect()),
            (Some(a), None) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        };

        Activation { scripts }
    }
}
