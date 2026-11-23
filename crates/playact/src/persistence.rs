//! Serializable blackboard snapshots.
//!
//! The blackboard deliberately erases value types at runtime. Persistence is
//! therefore opt-in: the application registers the typed [`Key`] values it wants
//! to save. Unknown keys and malformed values are rejected instead of being
//! silently ignored. Node state is persisted separately through
//! [`crate::BehaviorTree::snapshot`].

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use std::collections::BTreeMap;
use std::error::Error;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::blackboard::{Blackboard, Key};

/// Current blackboard snapshot format version.
pub const BLACKBOARD_SNAPSHOT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Payload {
    version: u32,
    values: BTreeMap<String, Value>,
}

trait KeyCodec: Send + Sync {
    fn encode(&self, board: &Blackboard) -> Result<Option<Value>, BlackboardCodecError>;
    fn decode(&self, board: &mut Blackboard, value: &Value) -> Result<(), BlackboardCodecError>;
}

struct TypedKeyCodec<T: Send + 'static> {
    key: Key<T>,
}

impl<T> KeyCodec for TypedKeyCodec<T>
where
    T: Serialize + DeserializeOwned + Send + 'static,
{
    fn encode(&self, board: &Blackboard) -> Result<Option<Value>, BlackboardCodecError> {
        let Some(value) = board.get(&self.key) else {
            return Ok(None);
        };

        serde_json::to_value(value)
            .map(Some)
            .map_err(|source| BlackboardCodecError::Encode {
                key: String::from(self.key.name()),
                source,
            })
    }

    fn decode(&self, board: &mut Blackboard, value: &Value) -> Result<(), BlackboardCodecError> {
        let decoded = serde_json::from_value(value.clone()).map_err(|source| {
            BlackboardCodecError::Decode {
                key: String::from(self.key.name()),
                source,
            }
        })?;
        board.set(&self.key, decoded);
        Ok(())
    }
}

/// Registry of typed blackboard keys used for persistence.
///
/// Register every key that should be saved or restored. The codec is reusable
/// and can encode/decode multiple blackboards; it never borrows a tree.
#[derive(Default)]
pub struct BlackboardCodec {
    keys: BTreeMap<&'static str, Box<dyn KeyCodec>>,
}

impl BlackboardCodec {
    /// Creates an empty codec.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a typed blackboard key.
    ///
    /// Returns an error when the same key name is registered twice.
    pub fn register<T>(&mut self, key: &Key<T>) -> Result<&mut Self, BlackboardCodecError>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
    {
        if self.keys.contains_key(key.name()) {
            return Err(BlackboardCodecError::DuplicateKey(String::from(key.name())));
        }

        self.keys
            .insert(key.name(), Box::new(TypedKeyCodec { key: *key }));
        Ok(self)
    }

    /// Number of registered keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether no keys are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Whether a key name is registered.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.keys.contains_key(name)
    }

    /// Serializes all registered values currently present on the blackboard.
    ///
    /// Missing keys are omitted from the output. Deserializing the result into a
    /// blackboard leaves those key slots untouched.
    pub fn encode(&self, board: &Blackboard) -> Result<Vec<u8>, BlackboardCodecError> {
        let mut values = BTreeMap::new();
        for (name, codec) in &self.keys {
            if let Some(value) = codec.encode(board)? {
                values.insert(String::from(*name), value);
            }
        }

        let payload = Payload {
            version: BLACKBOARD_SNAPSHOT_VERSION,
            values,
        };
        serde_json::to_vec(&payload).map_err(BlackboardCodecError::Serialize)
    }

    /// Restores values from [`BlackboardCodec::encode`].
    ///
    /// Unknown keys, unsupported versions, or wrong value types return an error.
    /// The blackboard may be partially updated before an error is returned.
    pub fn decode(&self, board: &mut Blackboard, bytes: &[u8]) -> Result<(), BlackboardCodecError> {
        let payload: Payload =
            serde_json::from_slice(bytes).map_err(BlackboardCodecError::Deserialize)?;

        if payload.version != BLACKBOARD_SNAPSHOT_VERSION {
            return Err(BlackboardCodecError::UnsupportedVersion {
                found: payload.version,
                supported: BLACKBOARD_SNAPSHOT_VERSION,
            });
        }

        for (name, value) in &payload.values {
            let Some(codec) = self.keys.get(name.as_str()) else {
                return Err(BlackboardCodecError::UnknownKey(name.clone()));
            };
            codec.decode(board, value)?;
        }

        Ok(())
    }
}

impl fmt::Debug for BlackboardCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlackboardCodec")
            .field("keys", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Errors produced while encoding or decoding a blackboard snapshot.
#[derive(Debug)]
#[non_exhaustive]
pub enum BlackboardCodecError {
    /// The same key name was registered more than once.
    DuplicateKey(String),
    /// The payload contains a key that is not registered.
    UnknownKey(String),
    /// The payload version is not supported.
    UnsupportedVersion {
        /// Version found in the payload.
        found: u32,
        /// Version supported by this build.
        supported: u32,
    },
    /// Top-level JSON serialization failed.
    Serialize(serde_json::Error),
    /// Top-level JSON deserialization failed.
    Deserialize(serde_json::Error),
    /// A registered value could not be encoded.
    Encode {
        /// Blackboard key name.
        key: String,
        /// Underlying serialization error.
        source: serde_json::Error,
    },
    /// A registered value could not be decoded.
    Decode {
        /// Blackboard key name.
        key: String,
        /// Underlying deserialization error.
        source: serde_json::Error,
    },
}

impl fmt::Display for BlackboardCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateKey(key) => write!(f, "blackboard key `{key}` is registered twice"),
            Self::UnknownKey(key) => write!(f, "blackboard payload contains unknown key `{key}`"),
            Self::UnsupportedVersion { found, supported } => write!(
                f,
                "unsupported blackboard snapshot version {found}; supported is {supported}"
            ),
            Self::Serialize(error) => write!(f, "could not serialize blackboard snapshot: {error}"),
            Self::Deserialize(error) => {
                write!(f, "could not deserialize blackboard snapshot: {error}")
            }
            Self::Encode { key, source } => {
                write!(f, "could not encode blackboard key `{key}`: {source}")
            }
            Self::Decode { key, source } => {
                write!(f, "could not decode blackboard key `{key}`: {source}")
            }
        }
    }
}

impl Error for BlackboardCodecError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Serialize(error) | Self::Deserialize(error) => Some(error),
            Self::Encode { source, .. } | Self::Decode { source, .. } => Some(source),
            Self::DuplicateKey(_) | Self::UnknownKey(_) | Self::UnsupportedVersion { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static COUNT: Key<u32> = Key::new("test.blackboard.count");
    static LABEL: Key<String> = Key::new("test.blackboard.label");

    #[test]
    fn roundtrip_registered_keys() {
        let mut codec = BlackboardCodec::new();
        codec.register(&COUNT).unwrap();
        codec.register(&LABEL).unwrap();

        let mut board = Blackboard::new();
        board.set(&COUNT, 42_u32);
        board.set(&LABEL, String::from("hello"));

        let bytes = codec.encode(&board).unwrap();
        let mut restored = Blackboard::new();
        codec.decode(&mut restored, &bytes).unwrap();

        assert_eq!(restored.get(&COUNT), Some(&42));
        assert_eq!(restored.get(&LABEL).map(String::as_str), Some("hello"));
    }

    #[test]
    fn missing_values_are_omitted_and_left_untouched() {
        let mut codec = BlackboardCodec::new();
        codec.register(&COUNT).unwrap();

        let board = Blackboard::new();
        let bytes = codec.encode(&board).unwrap();

        let mut restored = Blackboard::new();
        restored.set(&COUNT, 9_u32);
        codec.decode(&mut restored, &bytes).unwrap();
        assert_eq!(restored.get(&COUNT), Some(&9));
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let mut codec = BlackboardCodec::new();
        codec.register(&COUNT).unwrap();
        assert!(matches!(
            codec.register(&COUNT),
            Err(BlackboardCodecError::DuplicateKey(_))
        ));
    }

    #[test]
    fn unknown_keys_and_bad_types_are_rejected() {
        let mut codec = BlackboardCodec::new();
        codec.register(&COUNT).unwrap();
        let payload = br#"{"version":1,"values":{"test.blackboard.count":"oops"}}"#;
        assert!(matches!(
            codec.decode(&mut Blackboard::new(), payload),
            Err(BlackboardCodecError::Decode { .. })
        ));

        let unknown = br#"{"version":1,"values":{"test.blackboard.unknown":1}}"#;
        assert!(matches!(
            codec.decode(&mut Blackboard::new(), unknown),
            Err(BlackboardCodecError::UnknownKey(_))
        ));
    }

    #[test]
    fn unsupported_versions_are_rejected() {
        let codec = BlackboardCodec::new();
        let payload = br#"{"version":99,"values":{}}"#;
        assert!(matches!(
            codec.decode(&mut Blackboard::new(), payload),
            Err(BlackboardCodecError::UnsupportedVersion { .. })
        ));
    }
}
