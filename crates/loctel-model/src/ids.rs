//! Trace and span identifiers.

use std::fmt;

use serde::{Serialize, Serializer};

/// Why an identifier could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdError {
    /// `from_bytes` got the wrong number of bytes.
    WrongByteLength { expected: usize, actual: usize },
    /// `from_hex` got the wrong number of characters (twice the byte length
    /// is expected).
    WrongHexLength { expected: usize, actual: usize },
    /// `from_hex` met a character that is not `0-9`, `a-f` or `A-F`.
    InvalidHexDigit,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdError::WrongByteLength { expected, actual } => {
                write!(f, "expected {expected} bytes, got {actual}")
            }
            IdError::WrongHexLength { expected, actual } => {
                write!(f, "expected {expected} hex characters, got {actual}")
            }
            IdError::InvalidHexDigit => f.write_str("not a hexadecimal digit"),
        }
    }
}

impl std::error::Error for IdError {}

/// Lowercase hex of `bytes` (two characters per byte).
pub(crate) fn hex_string(bytes: &[u8]) -> String {
    use fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn hex_digit(byte: u8) -> Result<u8, IdError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(IdError::InvalidHexDigit),
    }
}

fn parse_hex<const N: usize>(text: &str) -> Result<[u8; N], IdError> {
    // Work on bytes so that non-ASCII input is an error, never a slicing
    // panic.
    let digits = text.as_bytes();
    if digits.len() != N * 2 {
        return Err(IdError::WrongHexLength {
            expected: N * 2,
            actual: digits.len(),
        });
    }
    let mut out = [0u8; N];
    // The length check above leaves no remainder.
    let (pairs, _) = digits.as_chunks::<2>();
    for (byte, [high, low]) in out.iter_mut().zip(pairs) {
        *byte = (hex_digit(*high)? << 4) | hex_digit(*low)?;
    }
    Ok(out)
}

fn copy_bytes<const N: usize>(bytes: &[u8]) -> Result<[u8; N], IdError> {
    <[u8; N]>::try_from(bytes).map_err(|_| IdError::WrongByteLength {
        expected: N,
        actual: bytes.len(),
    })
}

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $len:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub [u8; $len]);

        impl $name {
            /// Parses exactly twice the byte length in hex characters, in
            /// either case. All zeros parses fine; check [`Self::is_valid`].
            pub fn from_hex(text: &str) -> Result<Self, IdError> {
                parse_hex::<$len>(text).map(Self)
            }

            /// Builds the id from exactly the right number of raw bytes. All
            /// zeros is accepted; check [`Self::is_valid`].
            pub fn from_bytes(bytes: &[u8]) -> Result<Self, IdError> {
                copy_bytes::<$len>(bytes).map(Self)
            }

            /// OTLP uses all zeros for "no id", so that is the one invalid
            /// value.
            pub fn is_valid(&self) -> bool {
                self.0.iter().any(|&byte| byte != 0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&hex_string(&self.0))
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }
    };
}

define_id!(
    /// A 16-byte trace identifier. Displays and serializes as 32 lowercase
    /// hex characters.
    TraceId,
    16
);

define_id!(
    /// An 8-byte span identifier. Displays and serializes as 16 lowercase
    /// hex characters.
    SpanId,
    8
);
