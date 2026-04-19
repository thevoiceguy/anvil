//! # anvil-brand
//!
//! Desktop / generic implementations of [`BrandProvider`][bp] and
//! [`BrandCache`][bc] for the FCP brand-provisioning protocol. Mobile hosts
//! typically implement their own to use platform-native HTTP and secure
//! storage; this crate targets the desktop CLI and any host that just wants
//! something that works.
//!
//! See `docs/BRANDING.md` in the workspace root for the wire protocol.
//!
//! [bp]: anvil_core::BrandProvider
//! [bc]: anvil_core::BrandCache

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "http")]
mod http_provider;

#[cfg(feature = "fs-cache")]
mod fs_cache;

#[cfg(feature = "http")]
pub use http_provider::HttpBrandProvider;

#[cfg(feature = "fs-cache")]
pub use fs_cache::FsBrandCache;
