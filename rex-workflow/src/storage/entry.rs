use blake3::Hash;
use rex::Rex;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error as DeserializeError, MapAccess, Visitor},
    ser::SerializeMap,
};

/// The storage object represented by a tree entry.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize, Rex)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// An immutable byte blob.
    Blob,
    /// An immutable directory tree containing more named entries.
    Tree,
}

impl std::fmt::Display for EntryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            EntryKind::Blob => write!(f, "blob"),
            EntryKind::Tree => write!(f, "tree"),
        }
    }
}

/// Metadata for one immediate child of a content-addressed directory tree.
#[derive(Clone, Debug, Eq, PartialEq, Rex)]
pub struct Entry {
    /// The BLAKE3 content hash of the child blob or tree.
    pub hash: Hash,
    /// Whether the child is a blob or another tree.
    pub kind: EntryKind,
    /// The child's byte size as recorded by the storage backend.
    pub size: u64,
}

impl Serialize for Entry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("hash", self.hash.to_hex().as_str())?;
        map.serialize_entry("kind", &self.kind)?;
        map.serialize_entry("size", &self.size)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct EntryVisitor;

        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = Entry;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter
                    .write_str("a map containing a hexadecimal hash, an entry kind, and a size")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut hash = None;
                let mut kind = None;
                let mut size = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "hash" => {
                            if hash.is_some() {
                                return Err(A::Error::duplicate_field("hash"));
                            }
                            let hex = map.next_value::<String>()?;
                            hash = Some(Hash::from_hex(hex).map_err(A::Error::custom)?);
                        }
                        "kind" => {
                            if kind.is_some() {
                                return Err(A::Error::duplicate_field("kind"));
                            }
                            kind = Some(map.next_value()?);
                        }
                        "size" => {
                            if size.is_some() {
                                return Err(A::Error::duplicate_field("size"));
                            }
                            size = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(A::Error::unknown_field(&key, &["hash", "kind", "size"]));
                        }
                    }
                }

                let hash = hash.ok_or_else(|| A::Error::missing_field("hash"))?;
                let kind = kind.ok_or_else(|| A::Error::missing_field("kind"))?;
                let size = size.ok_or_else(|| A::Error::missing_field("size"))?;
                Ok(Entry { kind, hash, size })
            }
        }

        deserializer.deserialize_map(EntryVisitor)
    }
}
