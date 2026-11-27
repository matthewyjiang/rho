//! One-time HPKE (RFC 9180) recipient keys, and sealing to them. Knows
//! nothing of what is sealed: the caller supplies `info`, `aad`, and the
//! plaintext.
//!
//! The suite is DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305,
//! in base mode. Keys and sealed bytes travel as standard base64.
//!
//! A private key lives only in [`Recipients`], in memory, and goes at the
//! first attempt to open a bundle sealed to it, whether the bundle opens or
//! not, once [`RECIPIENT_LIFETIME`] passes, or when the process ends. A key
//! that is used up or expired is dropped, which zeroizes it.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use hpke::{
    aead::ChaCha20Poly1305, kdf::HkdfSha256, kem::X25519HkdfSha256, Deserializable, Kem as _,
    OpModeR, OpModeS, Serializable,
};
use zeroize::Zeroizing;

type Kem = X25519HkdfSha256;
type PrivateKey = <Kem as hpke::Kem>::PrivateKey;
type PublicKey = <Kem as hpke::Kem>::PublicKey;
type EncappedKey = <Kem as hpke::Kem>::EncappedKey;

/// How long an issued key can open a bundle.
//
// Policy, from the approved sync design: a client asks for the key right
// before it exports from the source, so the round trip takes seconds; five
// minutes leaves room for a slow ssh link without keeping keys around.
pub(super) const RECIPIENT_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// What [`seal`] produced: HPKE's encapsulated key and the sealed bytes
/// with their tag, each standard base64.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Sealed {
    pub(super) encapped_key: String,
    pub(super) ciphertext: String,
}

/// Why [`Recipients::open`] opened nothing.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum OpenError {
    /// No key is held for the recipient: never issued here, used up,
    /// expired, or issued before the process restarted.
    #[error("this daemon holds no key for that recipient; it expired, was used, or the daemon restarted")]
    Gone,
    /// The key was held, and is now used up, but the bundle did not open:
    /// it was not sealed to it, or was altered.
    #[error("the sealed bundle does not open: {0}")]
    Invalid(&'static str),
}

/// Why [`seal`] sealed nothing.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum SealError {
    #[error("the recipient is not an X25519 public key in standard base64")]
    InvalidRecipient,
    #[error("sealing failed")]
    Failed,
}

/// The keys this process issued that have not been used up or expired,
/// by their public key as [`Recipients::issue`] returned it.
#[derive(Default)]
pub(super) struct Recipients {
    held: Mutex<HashMap<String, Held>>,
}

struct Held {
    private: PrivateKey,
    expires: Instant,
}

impl Recipients {
    /// Makes a key pair, keeps its private key until [`RECIPIENT_LIFETIME`]
    /// after `now`, and returns its public key.
    pub(super) fn issue(&self, now: Instant) -> String {
        let (private, public) = Kem::gen_keypair();
        let public = BASE64.encode(public.to_bytes());
        let mut held = self.held.lock().expect("recipients lock");
        held.retain(|_, key| key.expires > now);
        held.insert(
            public.clone(),
            Held {
                private,
                expires: now + RECIPIENT_LIFETIME,
            },
        );
        public
    }

    /// Opens `sealed`, sealed to `recipient` with `info` and `aad`. Any
    /// attempt on a held key uses it up.
    pub(super) fn open(
        &self,
        now: Instant,
        recipient: &str,
        sealed: &Sealed,
        info: &[u8],
        aad: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, OpenError> {
        let key = {
            let mut held = self.held.lock().expect("recipients lock");
            held.retain(|_, key| key.expires > now);
            held.remove(recipient).ok_or(OpenError::Gone)?
        };
        let encapped = BASE64
            .decode(&sealed.encapped_key)
            .ok()
            .and_then(|bytes| EncappedKey::from_bytes(&bytes).ok())
            .ok_or(OpenError::Invalid("the encapsulated key is malformed"))?;
        let ciphertext = BASE64
            .decode(&sealed.ciphertext)
            .map_err(|_| OpenError::Invalid("the ciphertext is not standard base64"))?;
        hpke::single_shot_open::<ChaCha20Poly1305, HkdfSha256, Kem>(
            &OpModeR::Base,
            &key.private,
            &encapped,
            info,
            &ciphertext,
            aad,
        )
        .map(Zeroizing::new)
        .map_err(|_| OpenError::Invalid("it was not sealed to this recipient, or was altered"))
    }
}

/// Seals `plaintext` to `recipient`, a public key [`Recipients::issue`]
/// returned, with `info` and `aad`.
pub(super) fn seal(
    recipient: &str,
    info: &[u8],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Sealed, SealError> {
    let public = BASE64
        .decode(recipient)
        .ok()
        .and_then(|bytes| PublicKey::from_bytes(&bytes).ok())
        .ok_or(SealError::InvalidRecipient)?;
    let (encapped, ciphertext) = hpke::single_shot_seal::<ChaCha20Poly1305, HkdfSha256, Kem>(
        &OpModeS::Base,
        &public,
        info,
        plaintext,
        aad,
    )
    .map_err(|_| SealError::Failed)?;
    Ok(Sealed {
        encapped_key: BASE64.encode(encapped.to_bytes()),
        ciphertext: BASE64.encode(ciphertext),
    })
}

#[cfg(test)]
#[path = "seal_tests.rs"]
mod tests;
