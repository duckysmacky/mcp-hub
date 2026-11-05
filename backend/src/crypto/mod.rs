//! The only module that touches AEAD, KDFs, and raw key bytes (SECRETS.md).
//!
//! Password functions are slow on purpose: call them through `spawn_blocking`.

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const KEY_BYTES: usize = 32;
const SALT_BYTES: usize = 16;
const NONCE_BYTES: usize = 24;
const WRAP_AAD_PREFIX: &[u8] = b"mcp-hub/wrap/v1";
const ENV_AAD_PREFIX: &[u8] = b"mcp-hub/env/v1";

pub const ACCESS_TOKEN_PREFIX: &str = "mcphub_at_";
pub const REFRESH_TOKEN_PREFIX: &str = "mcphub_rt_";
pub const INVITE_CODE_PREFIX: &str = "mcphub_inv_";

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("decryption failed")]
    Decrypt,
    #[error("encryption failed")]
    Encrypt,
    #[error("key derivation failed")]
    KeyDerivation,
    #[error("password hashing failed")]
    PasswordHash,
}

/// A user's data encryption key. Deliberately neither `Clone` nor `Debug`.
pub struct Dek(Zeroizing<[u8; KEY_BYTES]>);

impl Dek {
    pub fn generate() -> Self {
        Self(Zeroizing::new(rand::random()))
    }
}

/// A key that only ever wraps a DEK.
pub struct Kek(Zeroizing<[u8; KEY_BYTES]>);

/// Which credential a DEK wrap belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapKind {
    Password,
    WebSession,
    AuthCode,
    AccessToken,
    RefreshToken,
}

impl WrapKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::WebSession => "web-session",
            Self::AuthCode => "auth-code",
            Self::AccessToken => "access-token",
            Self::RefreshToken => "refresh-token",
        }
    }
}

/// A random 256-bit credential. It is key material: never log or store it raw.
pub struct Token(Zeroizing<[u8; KEY_BYTES]>);

impl Token {
    pub fn generate() -> Self {
        Self(Zeroizing::new(rand::random()))
    }

    /// Parses `<prefix><base64url of 32 bytes>`. Anything else is `None`.
    pub fn parse(prefix: &str, text: &str) -> Option<Self> {
        let encoded = text.strip_prefix(prefix)?;
        let mut raw = Zeroizing::new([0u8; KEY_BYTES]);
        let decoded = Zeroizing::new(URL_SAFE_NO_PAD.decode(encoded).ok()?);
        if decoded.len() != KEY_BYTES {
            return None;
        }
        raw.copy_from_slice(&decoded);
        Some(Self(raw))
    }

    pub fn encode(&self, prefix: &str) -> Zeroizing<String> {
        Zeroizing::new(format!(
            "{prefix}{}",
            URL_SAFE_NO_PAD.encode(self.0.as_slice())
        ))
    }

    /// The value stored in the database and used for lookups.
    pub fn hash(&self) -> Vec<u8> {
        Sha256::digest(self.0.as_slice()).to_vec()
    }

    pub fn kek(&self, kind: WrapKind) -> Result<Kek, CryptoError> {
        let info = format!("mcp-hub/dek-wrap/v1/{}", kind.as_str());
        let mut key = Zeroizing::new([0u8; KEY_BYTES]);
        Hkdf::<Sha256>::new(None, &*self.0)
            .expand(info.as_bytes(), &mut *key)
            .map_err(|_| CryptoError::KeyDerivation)?;
        Ok(Kek(key))
    }
}

/// Argon2id cost parameters, stored per user so they can change later.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct KdfParams {
    #[serde(rename = "m")]
    pub memory_kib: u32,
    #[serde(rename = "t")]
    pub iterations: u32,
    #[serde(rename = "p")]
    pub parallelism: u32,
}

impl KdfParams {
    fn argon2(self) -> Result<Argon2<'static>, CryptoError> {
        let params = Params::new(self.memory_kib, self.iterations, self.parallelism, None)
            .map_err(|_| CryptoError::KeyDerivation)?;
        Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    }
}

pub fn generate_salt() -> Vec<u8> {
    rand::random::<[u8; SALT_BYTES]>().to_vec()
}

/// Derives the key that wraps a user's DEK. Unrelated to the verification hash.
pub fn derive_password_kek(
    password: &str,
    salt: &[u8],
    params: KdfParams,
) -> Result<Kek, CryptoError> {
    let mut key = Zeroizing::new([0u8; KEY_BYTES]);
    params
        .argon2()?
        .hash_password_into(password.as_bytes(), salt, &mut *key)
        .map_err(|_| CryptoError::KeyDerivation)?;
    Ok(Kek(key))
}

/// Creates the PHC string used only to verify a password. It has its own salt.
pub fn hash_password(password: &str, params: KdfParams) -> Result<String, CryptoError> {
    params
        .argon2()?
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|_| CryptoError::PasswordHash)
}

/// Checks a password using the parameters inside `phc`. A malformed hash never matches.
pub fn verify_password(password: &str, phc: &str) -> bool {
    PasswordHash::new(phc).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}

pub fn wrap_dek(
    kek: &Kek,
    dek: &Dek,
    kind: WrapKind,
    row_id: i64,
    user_id: i64,
) -> Result<Vec<u8>, CryptoError> {
    seal(&kek.0, &*dek.0, &wrap_aad(kind, row_id, user_id))
}

pub fn unwrap_dek(
    kek: &Kek,
    wrapped: &[u8],
    kind: WrapKind,
    row_id: i64,
    user_id: i64,
) -> Result<Dek, CryptoError> {
    let plaintext = open(&kek.0, wrapped, &wrap_aad(kind, row_id, user_id))?;
    let mut key = Zeroizing::new([0u8; KEY_BYTES]);
    if plaintext.len() != KEY_BYTES {
        return Err(CryptoError::Decrypt);
    }
    key.copy_from_slice(&plaintext);
    Ok(Dek(key))
}

pub fn encrypt_env(
    dek: &Dek,
    instance_id: i64,
    env_var: &str,
    value: &str,
) -> Result<Vec<u8>, CryptoError> {
    seal(&dek.0, value.as_bytes(), &env_aad(instance_id, env_var))
}

pub fn decrypt_env(
    dek: &Dek,
    instance_id: i64,
    env_var: &str,
    ciphertext: &[u8],
) -> Result<Zeroizing<String>, CryptoError> {
    let plaintext = open(&dek.0, ciphertext, &env_aad(instance_id, env_var))?;
    let text = std::str::from_utf8(&plaintext).map_err(|_| CryptoError::Decrypt)?;
    Ok(Zeroizing::new(text.to_owned()))
}

fn wrap_aad(kind: WrapKind, row_id: i64, user_id: i64) -> Vec<u8> {
    let mut aad = WRAP_AAD_PREFIX.to_vec();
    aad.push(0);
    aad.extend_from_slice(kind.as_str().as_bytes());
    aad.push(0);
    aad.extend_from_slice(&row_id.to_be_bytes());
    aad.extend_from_slice(&user_id.to_be_bytes());
    aad
}

fn env_aad(instance_id: i64, env_var: &str) -> Vec<u8> {
    let mut aad = ENV_AAD_PREFIX.to_vec();
    aad.push(0);
    aad.extend_from_slice(&instance_id.to_be_bytes());
    aad.extend_from_slice(env_var.as_bytes());
    aad
}

/// Returns `nonce || ciphertext || tag`.
fn seal(key: &[u8; KEY_BYTES], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| CryptoError::Encrypt)?;
    let nonce_bytes: [u8; NONCE_BYTES] = rand::random();
    let nonce = XNonce::from(nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Encrypt)?;
    let mut sealed = nonce_bytes.to_vec();
    sealed.extend_from_slice(&ciphertext);
    Ok(sealed)
}

fn open(
    key: &[u8; KEY_BYTES],
    sealed: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let (nonce_bytes, ciphertext) = sealed
        .split_at_checked(NONCE_BYTES)
        .ok_or(CryptoError::Decrypt)?;
    let nonce_bytes: [u8; NONCE_BYTES] =
        nonce_bytes.try_into().map_err(|_| CryptoError::Decrypt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| CryptoError::Decrypt)?;
    cipher
        .decrypt(
            &XNonce::from(nonce_bytes),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::Decrypt)
}

/// Checks a PKCE verifier against its S256 challenge (RFC 7636).
pub fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    let is_well_formed = (43..=128).contains(&verifier.len())
        && verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c));
    is_well_formed && URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == challenge
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_the_rfc_7636_example() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(pkce_matches(verifier, challenge));
        assert!(!pkce_matches(
            "wrong-verifier-wrong-verifier-wrong-verifier-1",
            challenge
        ));
        assert!(!pkce_matches("short", challenge));
        assert!(!pkce_matches(&"a".repeat(129), challenge));
        assert!(!pkce_matches(&format!("{verifier} "), challenge));
    }

    const FAST: KdfParams = KdfParams {
        memory_kib: 256,
        iterations: 1,
        parallelism: 1,
    };

    fn token_kek() -> Kek {
        Token::generate().kek(WrapKind::WebSession).unwrap()
    }

    /// A DEK can only be compared through what it decrypts.
    fn assert_same_dek(first: &Dek, second: &Dek) {
        let sealed = encrypt_env(first, 1, "VAR", "value").unwrap();
        assert_eq!(
            decrypt_env(second, 1, "VAR", &sealed).unwrap().as_str(),
            "value"
        );
    }

    #[test]
    fn wrap_round_trips() {
        let (kek, dek) = (token_kek(), Dek::generate());
        let wrapped = wrap_dek(&kek, &dek, WrapKind::WebSession, 5, 9).unwrap();
        assert_eq!(wrapped.len(), NONCE_BYTES + KEY_BYTES + 16);
        let unwrapped = unwrap_dek(&kek, &wrapped, WrapKind::WebSession, 5, 9).unwrap();
        assert_same_dek(&dek, &unwrapped);
    }

    #[test]
    fn wrap_cannot_move_between_rows_users_kinds_or_keys() {
        let (kek, dek) = (token_kek(), Dek::generate());
        let wrapped = wrap_dek(&kek, &dek, WrapKind::WebSession, 5, 9).unwrap();

        assert!(unwrap_dek(&kek, &wrapped, WrapKind::WebSession, 6, 9).is_err());
        assert!(unwrap_dek(&kek, &wrapped, WrapKind::WebSession, 5, 8).is_err());
        assert!(unwrap_dek(&kek, &wrapped, WrapKind::AccessToken, 5, 9).is_err());
        assert!(unwrap_dek(&token_kek(), &wrapped, WrapKind::WebSession, 5, 9).is_err());
    }

    #[test]
    fn env_round_trips_and_cannot_move_between_rows_or_users() {
        let dek = Dek::generate();
        let sealed = encrypt_env(&dek, 1, "TOKEN", "s3cret").unwrap();

        assert_eq!(
            decrypt_env(&dek, 1, "TOKEN", &sealed).unwrap().as_str(),
            "s3cret"
        );
        assert!(decrypt_env(&dek, 2, "TOKEN", &sealed).is_err());
        assert!(decrypt_env(&dek, 1, "OTHER", &sealed).is_err());
        assert!(decrypt_env(&Dek::generate(), 1, "TOKEN", &sealed).is_err());
    }

    #[test]
    fn tampered_or_truncated_ciphertext_fails() {
        let dek = Dek::generate();
        let mut sealed = encrypt_env(&dek, 1, "TOKEN", "s3cret").unwrap();
        assert!(decrypt_env(&dek, 1, "TOKEN", &sealed[..10]).is_err());
        assert!(decrypt_env(&dek, 1, "TOKEN", &[]).is_err());
        let last = sealed.len() - 1;
        sealed[last] ^= 1;
        assert!(decrypt_env(&dek, 1, "TOKEN", &sealed).is_err());
    }

    #[test]
    fn nonces_are_fresh() {
        let dek = Dek::generate();
        assert_ne!(
            encrypt_env(&dek, 1, "VAR", "value").unwrap(),
            encrypt_env(&dek, 1, "VAR", "value").unwrap()
        );
    }

    #[test]
    fn token_text_round_trips_with_the_same_key_material() {
        let token = Token::generate();
        let text = token.encode(ACCESS_TOKEN_PREFIX);
        assert!(text.starts_with("mcphub_at_"));

        let parsed = Token::parse(ACCESS_TOKEN_PREFIX, &text).unwrap();
        assert_eq!(parsed.hash(), token.hash());
        assert_eq!(parsed.hash().len(), 32);

        let dek = Dek::generate();
        let wrapped = wrap_dek(
            &token.kek(WrapKind::AccessToken).unwrap(),
            &dek,
            WrapKind::AccessToken,
            1,
            1,
        )
        .unwrap();
        let kek = parsed.kek(WrapKind::AccessToken).unwrap();
        assert!(unwrap_dek(&kek, &wrapped, WrapKind::AccessToken, 1, 1).is_ok());
    }

    #[test]
    fn malformed_tokens_are_rejected() {
        let good = Token::generate().encode(ACCESS_TOKEN_PREFIX);
        assert!(Token::parse(REFRESH_TOKEN_PREFIX, &good).is_none());
        assert!(Token::parse(ACCESS_TOKEN_PREFIX, "mcphub_at_short").is_none());
        assert!(Token::parse(ACCESS_TOKEN_PREFIX, "mcphub_at_!!!").is_none());
        assert!(Token::parse(ACCESS_TOKEN_PREFIX, "").is_none());
        let too_long = format!("mcphub_at_{}", URL_SAFE_NO_PAD.encode([0u8; 33]));
        assert!(Token::parse(ACCESS_TOKEN_PREFIX, &too_long).is_none());
    }

    #[test]
    fn each_wrap_kind_gets_its_own_key_from_one_token() {
        let token = Token::generate();
        let dek = Dek::generate();
        let wrapped = wrap_dek(
            &token.kek(WrapKind::AccessToken).unwrap(),
            &dek,
            WrapKind::AccessToken,
            1,
            1,
        )
        .unwrap();
        let wrong = token.kek(WrapKind::RefreshToken).unwrap();
        assert!(unwrap_dek(&wrong, &wrapped, WrapKind::AccessToken, 1, 1).is_err());
    }

    #[test]
    fn password_hash_verifies_only_the_right_password() {
        let hash = hash_password("correct horse", FAST).unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse", &hash));
        assert!(!verify_password("wrong", &hash));
        assert!(!verify_password("correct horse", "not a hash"));
    }

    #[test]
    fn password_kek_depends_on_password_and_salt() {
        let (salt, other_salt) = (generate_salt(), generate_salt());
        let kek = derive_password_kek("password", &salt, FAST).unwrap();
        let dek = Dek::generate();
        let wrapped = wrap_dek(&kek, &dek, WrapKind::Password, 1, 1).unwrap();

        let same = derive_password_kek("password", &salt, FAST).unwrap();
        assert!(unwrap_dek(&same, &wrapped, WrapKind::Password, 1, 1).is_ok());
        let other_password = derive_password_kek("other", &salt, FAST).unwrap();
        assert!(unwrap_dek(&other_password, &wrapped, WrapKind::Password, 1, 1).is_err());
        let other = derive_password_kek("password", &other_salt, FAST).unwrap();
        assert!(unwrap_dek(&other, &wrapped, WrapKind::Password, 1, 1).is_err());
    }

    #[test]
    fn kdf_params_use_the_documented_json_shape() {
        let json = serde_json::to_string(&FAST).unwrap();
        assert_eq!(json, r#"{"m":256,"t":1,"p":1}"#);
    }
}
