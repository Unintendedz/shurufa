//! The C ABI other IME hosts link against.
//!
//! Every function here is a thin shim: it validates the pointers and lengths it
//! was handed, calls into the crate's own logic, and hands back an owned JSON
//! string the caller frees with `msime_client_string_free`. Keeping them in one
//! file separates "what the library does" from "how a foreign caller reaches
//! it", which is the boundary the header in include/ describes.
//!
//! `#[no_mangle]` exports the symbol regardless of which module it sits in, so
//! moving these out of the crate root does not change the ABI.

use crate::*;

pub(crate) fn serialized_runtime_view(session: &HostSession) -> Result<Value, String> {
    serde_json::to_value(session.runtime.view()).map_err(|error| error.to_string())
}

pub(crate) unsafe fn with_bounded_bytes<T>(
    pointer: *const u8,
    length: usize,
    maximum: usize,
    invalid: &'static str,
    operation: impl FnOnce(&[u8]) -> Result<T, String>,
) -> Result<T, String> {
    if pointer.is_null() || length > maximum {
        return Err(invalid.into());
    }
    operation(unsafe { std::slice::from_raw_parts(pointer, length) })
}

pub(crate) fn absolute_path(value: &str) -> bool {
    Path::new(value).is_absolute()
}

#[cfg(unix)]
pub(crate) fn parse_absolute_socket_path(bytes: &[u8]) -> Result<&str, String> {
    parse_absolute_path(
        bytes,
        "socket path is not UTF-8",
        "socket path must be absolute",
    )
}

pub(crate) fn parse_absolute_path<'a>(
    bytes: &'a [u8],
    invalid_utf8: &'static str,
    non_absolute: &'static str,
) -> Result<&'a str, String> {
    let path = std::str::from_utf8(bytes).map_err(|_| invalid_utf8.to_owned())?;
    absolute_path(path)
        .then_some(path)
        .ok_or_else(|| non_absolute.to_owned())
}

pub mod account;
pub mod ai_skin;
pub mod candidates;
pub mod chat_stream;
pub mod cloud_dictionary;
pub mod cloud_sync;
pub mod community;
pub mod credentials;
pub mod dictionary_sync;
pub mod google;
pub mod host;
pub mod input;
pub mod lifecycle;
pub mod mcp;
pub mod moderation;
pub mod neural;
pub mod pack_downloads;
pub mod plugins;
pub mod providers;
pub mod reporting;
pub mod session;
pub mod session_store;
pub mod skin_image;
pub mod spacing;
pub mod translation;
pub mod voice;
pub mod voice_archive;

// Every export has always been reachable at the crate root; the domain split
// below is for readers, not for callers, so each module is flattened back out.
pub use candidates::*;
pub use credentials::*;
pub use dictionary_sync::*;
pub use host::*;
pub use input::*;
pub use lifecycle::*;
pub use mcp::*;
pub use pack_downloads::*;
pub use plugins::*;
pub use providers::*;
pub use session::*;
pub use skin_image::*;
pub use translation::*;
pub use voice::*;
pub use voice_archive::*;
