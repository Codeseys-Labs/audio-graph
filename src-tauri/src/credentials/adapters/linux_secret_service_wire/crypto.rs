//! The Secret Service DH session algorithm.

use aes::Aes128;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use crypto_bigint::{U256, U1024, const_monty_params, modular::ConstMontyForm};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

#[cfg(target_os = "linux")]
pub(super) const ALGORITHM: &str = "dh-ietf1024-sha256-aes128-cbc-pkcs7";
const PUBLIC_BYTES: usize = 128;
const AES_BYTES: usize = 16;
const GENERATOR: U1024 = U1024::from_u8(2);
const PRIME_HEX: &str = concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234",
    "C4C6628B80DC1CD129024E088A67CC74",
    "020BBEA63B139B22514A08798E3404DD",
    "EF9519B3CD3A431B302B0A6DF25F1437",
    "4FE1356D6D51C245E485B576625E7EC6",
    "F44C42E9A637ED6B0BFF5CB6F406B7ED",
    "EE386BFB5A899FA5AE9F24117C4B1FE6",
    "49286651ECE65381FFFFFFFFFFFFFFFF",
);
const PRIME: U1024 = U1024::from_be_hex(PRIME_HEX);
const SUBGROUP_ORDER: U1024 = PRIME.shr_vartime(1);
const_monty_params!(OakleyGroupTwo, U1024, PRIME_HEX);
type GroupElement = ConstMontyForm<OakleyGroupTwo, { U1024::LIMBS }>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CryptoError {
    #[cfg(target_os = "linux")]
    EntropyUnavailable,
    InvalidPrivateKey,
    InvalidPeerKey,
    Derivation,
    InvalidCiphertext,
}

pub(super) struct Exchange {
    exponent: Zeroizing<U256>,
}

impl Exchange {
    #[cfg(target_os = "linux")]
    pub(super) fn generate() -> Result<Self, CryptoError> {
        let mut bytes = Zeroizing::new([0; 32]);
        fill_random(bytes.as_mut())?;
        Self::new(&bytes)
    }

    /// The caller supplies 256 uniform bits from the operating system CSPRNG.
    pub(super) fn new(bytes: &Zeroizing<[u8; 32]>) -> Result<Self, CryptoError> {
        let exponent = Zeroizing::new(U256::from_be_slice(bytes.as_ref()));
        if *exponent <= U256::ONE {
            return Err(CryptoError::InvalidPrivateKey);
        }
        Ok(Self { exponent })
    }

    pub(super) fn public_key(&self) -> [u8; PUBLIC_BYTES] {
        let public = GroupElement::new(&GENERATOR).pow(&self.exponent).retrieve();
        let mut bytes = [0; PUBLIC_BYTES];
        bytes.copy_from_slice(public.to_be_bytes().as_ref());
        bytes
    }

    pub(super) fn finish(self, peer_key: &[u8]) -> Result<SessionKey, CryptoError> {
        let peer = validate_peer_key(peer_key)?;
        // The default pow uses the entire type width and selects entries in
        // constant time with respect to the exponent.
        let shared = Zeroizing::new(peer.pow(&self.exponent));
        let shared_integer = Zeroizing::new(shared.retrieve());
        let shared_bytes = Zeroizing::new(shared_integer.to_be_bytes());
        let mut key = Zeroizing::new([0; AES_BYTES]);
        // RFC 2631 preserves the leading zeroes of the shared value.
        derive_key(None, shared_bytes.as_ref(), b"", key.as_mut())?;
        Ok(SessionKey(key))
    }
}

fn validate_peer_key(bytes: &[u8]) -> Result<GroupElement, CryptoError> {
    // The peer's public integer may contain redundant leading zeroes.
    let first_nonzero = bytes
        .iter()
        .position(|&byte| byte != 0)
        .ok_or(CryptoError::InvalidPeerKey)?;
    let bytes = &bytes[first_nonzero..];
    if bytes.len() > PUBLIC_BYTES {
        return Err(CryptoError::InvalidPeerKey);
    }
    let mut padded = [0; PUBLIC_BYTES];
    padded[PUBLIC_BYTES - bytes.len()..].copy_from_slice(bytes);
    let value = U1024::from_be_slice(&padded);
    if value <= U1024::ONE || value >= PRIME.wrapping_sub(&U1024::ONE) {
        return Err(CryptoError::InvalidPeerKey);
    }
    let peer = GroupElement::new(&value);
    if peer.pow(&SUBGROUP_ORDER).retrieve() != U1024::ONE {
        return Err(CryptoError::InvalidPeerKey);
    }
    Ok(peer)
}

fn derive_key(
    salt: Option<&[u8]>,
    input: &[u8],
    info: &[u8],
    output: &mut [u8],
) -> Result<(), CryptoError> {
    Hkdf::<Sha256>::new(salt, input)
        .expand(info, output)
        .map_err(|_| CryptoError::Derivation)
}

pub(super) struct SessionKey(Zeroizing<[u8; AES_BYTES]>);

impl SessionKey {
    #[cfg(target_os = "linux")]
    pub(super) fn encrypt(&self, data: &[u8]) -> Result<([u8; AES_BYTES], Vec<u8>), CryptoError> {
        let mut iv = [0; AES_BYTES];
        fill_random(&mut iv)?;
        Ok((iv, self.encrypt_with_iv(&iv, data)?))
    }

    fn encrypt_with_iv(&self, iv: &[u8; AES_BYTES], data: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let size = data
            .len()
            .checked_add(AES_BYTES)
            .ok_or(CryptoError::InvalidCiphertext)?;
        let mut buf = Zeroizing::new(vec![0; size]);
        buf[..data.len()].copy_from_slice(data);
        let cipher = cbc::Encryptor::<Aes128>::new_from_slices(self.0.as_ref(), iv)
            .map_err(|_| CryptoError::InvalidCiphertext)?;
        let len = cipher
            .encrypt_padded_mut::<Pkcs7>(&mut buf, data.len())
            .map_err(|_| CryptoError::InvalidCiphertext)?
            .len();
        Ok(buf[..len].to_vec())
    }

    pub(super) fn decrypt(
        &self,
        iv: &[u8],
        ciphertext: Vec<u8>,
    ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        let mut plaintext = Zeroizing::new(ciphertext);
        let cipher = cbc::Decryptor::<Aes128>::new_from_slices(self.0.as_ref(), iv)
            .map_err(|_| CryptoError::InvalidCiphertext)?;
        let len = cipher
            .decrypt_padded_mut::<Pkcs7>(&mut plaintext)
            .map_err(|_| CryptoError::InvalidCiphertext)?
            .len();
        plaintext.truncate(len);
        Ok(plaintext)
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn encrypt_for_test(&self, iv: &[u8; AES_BYTES], data: &[u8]) -> Vec<u8> {
        self.encrypt_with_iv(iv, data)
            .expect("test fixture fits a padded buffer")
    }
}

#[cfg(target_os = "linux")]
fn fill_random(bytes: &mut [u8]) -> Result<(), CryptoError> {
    getrandom::fill(bytes).map_err(|_| CryptoError::EntropyUnavailable)
}

#[cfg(test)]
mod tests;
