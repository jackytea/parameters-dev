use std::fmt;

use parameters_store::EncryptedSecret;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use uuid::Uuid;

const KEY_BYTES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("the encryption key must be {} hexadecimal characters", KEY_BYTES * 2)]
    Malformed,
    #[error("the system random number generator failed")]
    Random,
    #[error("a secret cannot be encrypted")]
    Encrypt,
    #[error(
        "a secret cannot be decrypted; it was stored with a different encryption key or was changed"
    )]
    Decrypt,
}

#[derive(Clone, PartialEq, Eq)]
pub struct EncryptionKey([u8; KEY_BYTES]);

impl EncryptionKey {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; KEY_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn from_hex(text: &str) -> Result<Self, KeyError> {
        let text = text.trim().as_bytes();
        if text.len() != KEY_BYTES * 2 {
            return Err(KeyError::Malformed);
        }
        let mut key = [0; KEY_BYTES];
        for (byte, [high, low]) in key.iter_mut().zip(text.as_chunks::<2>().0) {
            *byte = (nibble(*high)? << 4) | nibble(*low)?;
        }
        Ok(Self(key))
    }
}

impl fmt::Debug for EncryptionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EncryptionKey(••••)")
    }
}

const fn nibble(digit: u8) -> Result<u8, KeyError> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        b'A'..=b'F' => Ok(digit - b'A' + 10),
        _ => Err(KeyError::Malformed),
    }
}

pub(crate) struct Cipher {
    key: LessSafeKey,
    random: SystemRandom,
}

impl Cipher {
    pub(crate) fn new(key: &EncryptionKey) -> Result<Self, KeyError> {
        let key = UnboundKey::new(&AES_256_GCM, &key.0).map_err(|_| KeyError::Malformed)?;
        Ok(Self {
            key: LessSafeKey::new(key),
            random: SystemRandom::new(),
        })
    }

    pub(crate) fn random_uuid(&self) -> Result<Uuid, KeyError> {
        let mut bytes = [0; 16];
        self.random.fill(&mut bytes).map_err(|_| KeyError::Random)?;
        Ok(uuid::Builder::from_random_bytes(bytes).into_uuid())
    }

    pub(crate) fn seal(
        &self,
        owner: Uuid,
        name: &str,
        plaintext: &str,
    ) -> Result<EncryptedSecret, KeyError> {
        let mut nonce = [0; NONCE_LEN];
        self.random.fill(&mut nonce).map_err(|_| KeyError::Random)?;
        let mut ciphertext = plaintext.as_bytes().to_vec();
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(associated_data(owner, name)),
                &mut ciphertext,
            )
            .map_err(|_| KeyError::Encrypt)?;
        Ok(EncryptedSecret {
            name: name.to_owned(),
            nonce: nonce.to_vec(),
            ciphertext,
        })
    }

    pub(crate) fn open(&self, owner: Uuid, secret: &EncryptedSecret) -> Result<String, KeyError> {
        let nonce =
            Nonce::try_assume_unique_for_key(&secret.nonce).map_err(|_| KeyError::Decrypt)?;
        let mut buffer = secret.ciphertext.clone();
        let plaintext = self
            .key
            .open_in_place(
                nonce,
                Aad::from(associated_data(owner, &secret.name)),
                &mut buffer,
            )
            .map_err(|_| KeyError::Decrypt)?;
        String::from_utf8(plaintext.to_vec()).map_err(|_| KeyError::Decrypt)
    }
}

fn associated_data(owner: Uuid, name: &str) -> Vec<u8> {
    let mut data = Vec::with_capacity(16 + name.len());
    data.extend_from_slice(owner.as_bytes());
    data.extend_from_slice(name.as_bytes());
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher(byte: u8) -> Cipher {
        Cipher::new(&EncryptionKey::from_bytes([byte; KEY_BYTES])).unwrap()
    }

    #[test]
    fn parses_hex_keys_and_hides_them() {
        let key = EncryptionKey::from_hex(&"aB".repeat(32)).unwrap();
        assert_eq!(key, EncryptionKey::from_bytes([0xab; 32]));
        assert_eq!(format!("{key:?}"), "EncryptionKey(••••)");
        for bad in ["", "ab", &"zz".repeat(32), &"ab".repeat(33)] {
            assert_eq!(EncryptionKey::from_hex(bad), Err(KeyError::Malformed));
        }
    }

    #[test]
    fn seals_and_opens_with_owner_and_name_bound() {
        let cipher = cipher(7);
        let owner = Uuid::from_u128(9);
        let sealed = cipher.seal(owner, "TOKEN", "hunter2").unwrap();
        assert!(
            !sealed
                .ciphertext
                .windows(7)
                .any(|window| window == b"hunter2")
        );
        assert_eq!(cipher.open(owner, &sealed).unwrap(), "hunter2");
        let again = cipher.seal(owner, "TOKEN", "hunter2").unwrap();
        assert_ne!(again.nonce, sealed.nonce);
        let renamed = EncryptedSecret {
            name: "OTHER".into(),
            ..sealed.clone()
        };
        assert_eq!(cipher.open(owner, &renamed), Err(KeyError::Decrypt));
        assert_eq!(
            cipher.open(Uuid::from_u128(10), &sealed),
            Err(KeyError::Decrypt)
        );
        assert_eq!(
            super::tests::cipher(8).open(owner, &sealed),
            Err(KeyError::Decrypt)
        );
    }
}
