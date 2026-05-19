//! Content-addressed storage: blobs, trees, commits, refs.
//!
//! Modelled after Git's object store, mapped into SQLite tables. The hashing
//! function is SHA-256 (not Git's SHA-1) — adequate for the next decade and
//! avoids the SHA-1 weaknesses.

pub mod blob;
pub mod commit;
pub mod refs;
pub mod tree;

pub use blob::{Blob, get_blob, put_blob};
pub use commit::{Commit, put_commit};
pub use refs::{Ref, RefKind, get_ref, list_refs, set_ref};
pub use tree::{Tree, TreeEntry, get_tree, put_tree};

use sha2::{Digest, Sha256};

/// Hex-encoded SHA-256 of the given bytes.
#[must_use]
pub fn hash(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}
