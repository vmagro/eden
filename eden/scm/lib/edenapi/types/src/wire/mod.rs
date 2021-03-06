/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

//! This module contains wire representation structs for external types we'd
//! like to avoid explicitly depending on. Types will be added as they are
//! used.
//!
//! These types should all be `pub(crate)`. They're used extensively inside the
//! crate, but should never appear outside it. The methods on the request /
//! response objects should accept and return the public types from
//! `eden/scm/lib/types`.
//!
//! To maintain wire-protocol compatibility, we have some important conventions
//! and requirements for all types defined inside this module:
//!
//! 1. Every field should be renamed to a unique natural number using
//! `#[serde(rename = "0")]`. New fields should never re-use a field identifier
//! that has been used before. If a field changes semantically, it should be
//! considered a new field, and be given a new identifier.
//!
//! 2. Every enum should have an "Unknown" variant as the last variant in the
//! enum. This variant should be annotated with `#[serde(other, rename = "0")]`
//!
//! 3. When practical, fields should be annotated with
//! `#[serde(default, skip_serializing_if = "is_default")` to save space on the
//! wire. Do not use `#[serde(default)]` on the container.
//!
//! 4. All fields should be wrapped in `Option` or in a container that may be
//! empty, such as `Vec`. If an empty container has special semantics (other
//! than ignoring the field), please wrap that field in an `Option` as well to
//! distinguish between "empty" and "not present".

pub mod complete_tree;
pub mod file;
pub mod tree;

pub use crate::wire::{
    complete_tree::WireCompleteTreeRequest,
    file::{WireFileEntry, WireFileRequest},
    tree::{WireTreeEntry, WireTreeRequest},
};

use std::convert::Infallible;
use std::fmt;

#[cfg(any(test, feature = "for-tests"))]
use quickcheck::Arbitrary;
use serde_derive::{Deserialize, Serialize};
use thiserror::Error;

use revisionstore_types::Metadata as RevisionstoreMetadata;
use types::{path::ParseError as RepoPathParseError, HgId, Key, Parents, RepoPathBuf};

#[derive(Debug, Error)]
#[error("Failed to convert from wire to API representation")]
pub enum WireToApiConversionError {
    UnrecognizedEnumVariant(&'static str),
    CannotPopulateRequiredField(&'static str),
    PathValidationError(RepoPathParseError),
}

impl From<Infallible> for WireToApiConversionError {
    fn from(v: Infallible) -> Self {
        match v {}
    }
}

impl From<RepoPathParseError> for WireToApiConversionError {
    fn from(v: RepoPathParseError) -> Self {
        WireToApiConversionError::PathValidationError(v)
    }
}

/// Convert from an EdenAPI API type to Wire type
pub trait ToWire: Sized {
    type Wire: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug + ToApi<Api = Self>;

    fn to_wire(self) -> Self::Wire;
}

/// Covnert from an EdenAPI Wire type to API type
pub trait ToApi: Sized {
    type Api;
    type Error: Into<WireToApiConversionError> + fmt::Debug;

    fn to_api(self) -> Result<Self::Api, Self::Error>;
}

impl<A: ToWire> ToWire for Vec<A> {
    type Wire = Vec<<A as ToWire>::Wire>;

    fn to_wire(self) -> Self::Wire {
        let mut out = Vec::with_capacity(self.len());
        for v in self.into_iter() {
            out.push(v.to_wire())
        }
        out
    }
}

impl<W: ToApi> ToApi for Vec<W> {
    type Api = Vec<<W as ToApi>::Api>;
    type Error = <W as ToApi>::Error;

    fn to_api(self) -> Result<Self::Api, Self::Error> {
        let mut out = Vec::with_capacity(self.len());
        for v in self.into_iter() {
            out.push(v.to_api()?)
        }
        Ok(out)
    }
}

impl<A: ToWire> ToWire for Option<A> {
    type Wire = Option<<A as ToWire>::Wire>;

    fn to_wire(self) -> Self::Wire {
        self.map(|a| a.to_wire())
    }
}

impl<W: ToApi> ToApi for Option<W> {
    type Api = Option<<W as ToApi>::Api>;
    type Error = <W as ToApi>::Error;

    fn to_api(self) -> Result<Self::Api, Self::Error> {
        self.map(|w| w.to_api()).transpose()
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireHgId(
    #[serde(rename = "0", default, skip_serializing_if = "is_default")] [u8; WireHgId::len()],
);

impl WireHgId {
    const fn len() -> usize {
        20
    }
}

impl ToWire for HgId {
    type Wire = WireHgId;

    fn to_wire(self) -> Self::Wire {
        WireHgId(self.into_byte_array())
    }
}

impl ToApi for WireHgId {
    type Api = HgId;
    type Error = Infallible;

    fn to_api(self) -> Result<Self::Api, Self::Error> {
        Ok(HgId::from_byte_array(self.0))
    }
}

impl fmt::Display for WireHgId {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self.to_api() {
            Ok(api) => fmt::Display::fmt(&api, fmt),
            Err(_) => Err(fmt::Error),
        }
    }
}

impl fmt::Debug for WireHgId {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self.to_api() {
            Ok(api) => write!(fmt, "WireHgId({:?})", &api.to_hex()),
            Err(_) => Err(fmt::Error),
        }
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireRepoPathBuf(
    #[serde(rename = "0", default, skip_serializing_if = "is_default")] String,
);

impl ToWire for RepoPathBuf {
    type Wire = WireRepoPathBuf;

    fn to_wire(self) -> Self::Wire {
        WireRepoPathBuf(self.into_string())
    }
}

impl ToApi for WireRepoPathBuf {
    type Api = RepoPathBuf;
    type Error = RepoPathParseError;

    fn to_api(self) -> Result<Self::Api, Self::Error> {
        Ok(RepoPathBuf::from_string(self.0)?)
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireKey {
    #[serde(rename = "0", default, skip_serializing_if = "is_default")]
    path: WireRepoPathBuf,

    #[serde(rename = "1", default, skip_serializing_if = "is_default")]
    hgid: WireHgId,
}

impl ToWire for Key {
    type Wire = WireKey;

    fn to_wire(self) -> Self::Wire {
        WireKey {
            path: self.path.to_wire(),
            hgid: self.hgid.to_wire(),
        }
    }
}

impl ToApi for WireKey {
    type Api = Key;
    type Error = WireToApiConversionError;

    fn to_api(self) -> Result<Self::Api, Self::Error> {
        Ok(Key {
            path: self.path.to_api()?,
            hgid: self.hgid.to_api()?,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireParents {
    #[serde(rename = "1")]
    None,

    #[serde(rename = "2")]
    One(WireHgId),

    #[serde(rename = "3")]
    Two(WireHgId, WireHgId),

    #[serde(other, rename = "0")]
    Unknown,
}

impl ToWire for Parents {
    type Wire = WireParents;

    fn to_wire(self) -> Self::Wire {
        use Parents::*;
        match self {
            None => WireParents::None,
            One(id) => WireParents::One(id.to_wire()),
            Two(id1, id2) => WireParents::Two(id1.to_wire(), id2.to_wire()),
        }
    }
}

impl ToApi for WireParents {
    type Api = Parents;
    type Error = WireToApiConversionError;

    fn to_api(self) -> Result<Self::Api, Self::Error> {
        use WireParents::*;
        Ok(match self {
            Unknown => {
                return Err(WireToApiConversionError::UnrecognizedEnumVariant(
                    "WireParents",
                ));
            }
            None => Parents::None,
            One(id) => Parents::One(id.to_api()?),
            Two(id1, id2) => Parents::Two(id1.to_api()?, id2.to_api()?),
        })
    }
}

impl Default for WireParents {
    fn default() -> Self {
        WireParents::None
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireRevisionstoreMetadata {
    #[serde(rename = "0", default, skip_serializing_if = "is_default")]
    size: Option<u64>,

    #[serde(rename = "1", default, skip_serializing_if = "is_default")]
    flags: Option<u64>,
}

impl ToWire for RevisionstoreMetadata {
    type Wire = WireRevisionstoreMetadata;

    fn to_wire(self) -> Self::Wire {
        WireRevisionstoreMetadata {
            size: self.size,
            flags: self.flags,
        }
    }
}

impl ToApi for WireRevisionstoreMetadata {
    type Api = RevisionstoreMetadata;
    type Error = Infallible;

    fn to_api(self) -> Result<Self::Api, Self::Error> {
        Ok(RevisionstoreMetadata {
            size: self.size,
            flags: self.flags,
        })
    }
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    v == &T::default()
}

#[cfg(any(test, feature = "for-tests"))]
impl Arbitrary for WireHgId {
    fn arbitrary<G: quickcheck::Gen>(g: &mut G) -> Self {
        HgId::arbitrary(g).to_wire()
    }
}

#[cfg(any(test, feature = "for-tests"))]
impl Arbitrary for WireRepoPathBuf {
    fn arbitrary<G: quickcheck::Gen>(g: &mut G) -> Self {
        RepoPathBuf::arbitrary(g).to_wire()
    }
}

#[cfg(any(test, feature = "for-tests"))]
impl Arbitrary for WireKey {
    fn arbitrary<G: quickcheck::Gen>(g: &mut G) -> Self {
        Key::arbitrary(g).to_wire()
    }
}

#[cfg(any(test, feature = "for-tests"))]
impl Arbitrary for WireParents {
    fn arbitrary<G: quickcheck::Gen>(g: &mut G) -> Self {
        Parents::arbitrary(g).to_wire()
    }
}

#[cfg(any(test, feature = "for-tests"))]
impl Arbitrary for WireRevisionstoreMetadata {
    fn arbitrary<G: quickcheck::Gen>(g: &mut G) -> Self {
        Self {
            size: Arbitrary::arbitrary(g),
            flags: Arbitrary::arbitrary(g),
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    use quickcheck::quickcheck;

    pub fn check_serialize_roundtrip<
        T: serde::Serialize + serde::de::DeserializeOwned + Clone + PartialEq,
    >(
        original: T,
    ) -> bool {
        let serial = serde_cbor::to_vec(&original).unwrap();
        let roundtrip = serde_cbor::from_slice(&serial).unwrap();
        original == roundtrip
    }

    pub fn check_wire_roundtrip<T: ToWire + Clone + PartialEq>(original: T) -> bool {
        let wire = original.clone().to_wire();
        let roundtrip = wire.to_api().unwrap();
        original == roundtrip
    }

    quickcheck! {
        fn test_hgid_roundtrip_serialize(v: WireHgId) -> bool {
            check_serialize_roundtrip(v)
        }

        fn test_hgid_roundtrip_wire(v: HgId) -> bool {
            check_wire_roundtrip(v)
        }

        fn test_key_roundtrip_serialize(v: WireKey) -> bool {
            check_serialize_roundtrip(v)
        }

        fn test_key_roundtrip_wire(v: Key) -> bool {
            check_wire_roundtrip(v)
        }

        fn test_path_roundtrip_serialize(v: WireRepoPathBuf) -> bool {
            check_serialize_roundtrip(v)
        }

        fn test_path_roundtrip_wire(v: RepoPathBuf) -> bool {
            check_wire_roundtrip(v)
        }

        fn test_parents_roundtrip_serialize(v: WireParents) -> bool {
            check_serialize_roundtrip(v)
        }

        fn test_parents_roundtrip_wire(v: Parents) -> bool {
            check_wire_roundtrip(v)
        }

        fn test_meta_roundtrip_serialize(v: WireRevisionstoreMetadata) -> bool {
            check_serialize_roundtrip(v)
        }

        fn test_meta_roundtrip_wire(v: RevisionstoreMetadata) -> bool {
            check_wire_roundtrip(v)
        }
    }
}
