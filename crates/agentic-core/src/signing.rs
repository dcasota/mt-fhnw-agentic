//! Post-quantum non-repudiation signing (ADR-0039: PQC-only).
//!
//! All signing uses **ML-DSA-87** (FIPS 204, NIST Category 5), matching the
//! thesis's own CNSA 2.0 alignment. Classical ciphers (Ed25519/RSA/ECDSA) are
//! forbidden by policy. Private keys never touch the database — they live in
//! the OS keychain; only public keys (`crypto_keys`) and detached signatures
//! (`signatures`) are persisted here.

use fips204::ml_dsa_87;
use fips204::traits::{SerDes, Signer, Verifier};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};
use sha2::{Digest, Sha256};

pub const ALG: &str = "ML-DSA-87";

/// A freshly generated ML-DSA-87 keypair, hex-encoded.
pub struct KeyPair {
    pub key_id: String,
    pub public_hex: String,
    pub secret_hex: String,
}

fn map_err(e: &str) -> Error {
    Error::Crypto(format!("ML-DSA-87: {e}"))
}

/// Short, stable identifier for a public key: first 16 hex chars of its SHA-256.
fn derive_key_id(public_hex: &str) -> String {
    let digest = Sha256::digest(public_hex.as_bytes());
    hex::encode(digest)[..16].to_owned()
}

/// SHA-256 of arbitrary bytes, hex-encoded (used to hash an audit report body
/// before signing it).
pub fn digest_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Generate a new ML-DSA-87 keypair (uses the OS RNG via `fips204`).
pub fn generate() -> Result<KeyPair> {
    let (pk, sk) = ml_dsa_87::try_keygen().map_err(map_err)?;
    let public_hex = hex::encode(pk.into_bytes());
    let secret_hex = hex::encode(sk.into_bytes());
    let key_id = derive_key_id(&public_hex);
    Ok(KeyPair {
        key_id,
        public_hex,
        secret_hex,
    })
}

/// Sign `msg` with a hex-encoded ML-DSA-87 secret key; returns a hex signature.
pub fn sign(secret_hex: &str, msg: &[u8]) -> Result<String> {
    let bytes = hex::decode(secret_hex).map_err(|e| Error::Crypto(format!("bad sk hex: {e}")))?;
    let arr: [u8; ml_dsa_87::SK_LEN] = bytes
        .try_into()
        .map_err(|_| Error::Crypto("secret key wrong length".into()))?;
    let sk = ml_dsa_87::PrivateKey::try_from_bytes(arr).map_err(map_err)?;
    let sig = sk.try_sign(msg, b"").map_err(map_err)?;
    Ok(hex::encode(sig))
}

/// Verify a hex signature over `msg` against a hex-encoded ML-DSA-87 public key.
pub fn verify(public_hex: &str, msg: &[u8], signature_hex: &str) -> Result<bool> {
    let pk_bytes =
        hex::decode(public_hex).map_err(|e| Error::Crypto(format!("bad pk hex: {e}")))?;
    let pk_arr: [u8; ml_dsa_87::PK_LEN] = pk_bytes
        .try_into()
        .map_err(|_| Error::Crypto("public key wrong length".into()))?;
    let pk = ml_dsa_87::PublicKey::try_from_bytes(pk_arr).map_err(map_err)?;
    let sig_bytes =
        hex::decode(signature_hex).map_err(|e| Error::Crypto(format!("bad sig hex: {e}")))?;
    let sig_arr: [u8; ml_dsa_87::SIG_LEN] = sig_bytes
        .try_into()
        .map_err(|_| Error::Crypto("signature wrong length".into()))?;
    Ok(pk.verify(msg, &sig_arr, b""))
}

// ---- DB-backed key + signature registry --------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CryptoKey {
    pub key_id: String,
    pub alg: String,
    pub public_key: String,
    pub sk_ref: String,
    pub signer: Option<String>,
    pub created_at: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    pub target_kind: String,
    pub target_id: String,
    pub alg: String,
    pub key_id: String,
    pub signature: String,
    pub signer: Option<String>,
    pub signed_at: String,
}

/// Register a public key (deactivating any previous active key). The secret key
/// itself is stored by the caller in the OS keychain under `sk_ref`.
pub fn register_key(
    conn: &Connection,
    key_id: &str,
    public_hex: &str,
    sk_ref: &str,
    signer: Option<&str>,
) -> Result<()> {
    conn.execute("UPDATE crypto_keys SET active = 0 WHERE active = 1", [])?;
    conn.execute(
        "INSERT OR REPLACE INTO crypto_keys (key_id, alg, public_key, sk_ref, signer, active) \
         VALUES (?1, ?2, ?3, ?4, ?5, 1)",
        params![key_id, ALG, public_hex, sk_ref, signer],
    )?;
    Ok(())
}

/// The currently active signing key, if any.
pub fn active_key(conn: &Connection) -> Result<Option<CryptoKey>> {
    conn.query_row(
        "SELECT key_id, alg, public_key, sk_ref, signer, created_at, active \
         FROM crypto_keys WHERE active = 1 ORDER BY created_at DESC LIMIT 1",
        [],
        |r| {
            Ok(CryptoKey {
                key_id: r.get(0)?,
                alg: r.get(1)?,
                public_key: r.get(2)?,
                sk_ref: r.get(3)?,
                signer: r.get(4)?,
                created_at: r.get(5)?,
                active: r.get::<_, i64>(6)? != 0,
            })
        },
    )
    .optional()
    .map_err(Error::from)
}

/// Look up a public key by its id (used for verification of historical sigs).
pub fn key_by_id(conn: &Connection, key_id: &str) -> Result<Option<CryptoKey>> {
    conn.query_row(
        "SELECT key_id, alg, public_key, sk_ref, signer, created_at, active \
         FROM crypto_keys WHERE key_id = ?1",
        params![key_id],
        |r| {
            Ok(CryptoKey {
                key_id: r.get(0)?,
                alg: r.get(1)?,
                public_key: r.get(2)?,
                sk_ref: r.get(3)?,
                signer: r.get(4)?,
                created_at: r.get(5)?,
                active: r.get::<_, i64>(6)? != 0,
            })
        },
    )
    .optional()
    .map_err(Error::from)
}

/// Record a detached signature over `target_id` (idempotent per key).
pub fn record_signature(
    conn: &Connection,
    target_kind: &str,
    target_id: &str,
    key_id: &str,
    signature_hex: &str,
    signer: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO signatures (target_kind, target_id, alg, key_id, signature, signer) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![target_kind, target_id, ALG, key_id, signature_hex, signer],
    )?;
    Ok(())
}

/// All signatures over a given target.
pub fn signatures_for(
    conn: &Connection,
    target_kind: &str,
    target_id: &str,
) -> Result<Vec<Signature>> {
    let mut stmt = conn.prepare(
        "SELECT target_kind, target_id, alg, key_id, signature, signer, signed_at \
         FROM signatures WHERE target_kind = ?1 AND target_id = ?2 ORDER BY signed_at",
    )?;
    let rows = stmt
        .query_map(params![target_kind, target_id], |r| {
            Ok(Signature {
                target_kind: r.get(0)?,
                target_id: r.get(1)?,
                alg: r.get(2)?,
                key_id: r.get(3)?,
                signature: r.get(4)?,
                signer: r.get(5)?,
                signed_at: r.get(6)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Count signatures by target kind (for the audit summary).
pub fn count_by_kind(conn: &Connection, target_kind: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM signatures WHERE target_kind = ?1",
        params![target_kind],
        |r| r.get(0),
    )
    .map_err(Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip() {
        let kp = generate().unwrap();
        let msg = b"non-repudiation test payload";
        let sig = sign(&kp.secret_hex, msg).unwrap();
        assert!(verify(&kp.public_hex, msg, &sig).unwrap());
        // Tampered message must fail.
        assert!(!verify(&kp.public_hex, b"different", &sig).unwrap());
    }

    #[test]
    fn key_id_is_stable() {
        let kp = generate().unwrap();
        assert_eq!(kp.key_id, derive_key_id(&kp.public_hex));
        assert_eq!(kp.key_id.len(), 16);
    }
}
