use std::fmt::{self, Display, Formatter};

use uuid::Uuid;

#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "lowercase")]
pub struct Id(pub Uuid);

impl Id {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for Id {
    fn default() -> Self {
        Self(Uuid::max())
    }
}

impl Display for Id {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (fst, snd, thrd, _frth) = self.0.as_fields();
        let id = (fst as u64) << 32 | (snd as u64) << 16 | (thrd as u64);
        id.fmt(f)
    }
}
