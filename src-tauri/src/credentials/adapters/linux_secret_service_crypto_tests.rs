use super::*;
use sha2::Digest;

fn bytes(hex: &str) -> Vec<u8> {
    hex::decode(hex).expect("published vector hex")
}

fn exchange(exponent: u16) -> Exchange {
    let mut private = Zeroizing::new([0; 32]);
    private[30..].copy_from_slice(&exponent.to_be_bytes());
    Exchange::new(&private).expect("valid test exponent")
}

#[test]
fn rfc2409_group_two() {
    // SHA-256 of the prime's 128 bytes as listed in RFC 2409 §6.2.
    let hash = bytes(concat!(
        "3f35a3f5f6c4376a744acad409bb22f8",
        "d897f949d2311d885adaa890981b67a0",
    ));
    assert_eq!(Sha256::digest(PRIME.to_be_bytes()).as_slice(), hash);
    assert_eq!(GENERATOR, U1024::from_u8(2));
    assert_eq!(PRIME.bit_width_vartime(), 1024);
}

#[test]
fn rfc5869_sha256_cases() {
    fn check(salt: Option<&[u8]>, input: &[u8], info: &[u8], expected: &str) {
        let expected = bytes(expected);
        let mut result = vec![0; expected.len()];
        derive_key(salt, input, info, &mut result).expect("valid test output length");
        assert_eq!(result, expected);
    }

    check(
        Some(&(0..=0x0c).collect::<Vec<_>>()),
        &[0x0b; 22],
        &(0xf0..=0xf9).collect::<Vec<_>>(),
        concat!(
            "3cb25f25faacd57a90434f64d0362f2a",
            "2d2d0a90cf1a5a4c5db02d56ecc4c5bf",
            "34007208d5b887185865"
        ),
    );
    check(
        Some(&(0x60..=0xaf).collect::<Vec<_>>()),
        &(0..=0x4f).collect::<Vec<_>>(),
        &(0xb0..=0xff).collect::<Vec<_>>(),
        concat!(
            "b11e398dc80327a1c8e7f78c596a4934",
            "4f012eda2d4efad8a050cc4c19afa97c",
            "59045a99cac7827271cb41c65e590e09",
            "da3275600c2f09b8367793a9aca3db71",
            "cc30c58179ec3e87c14c01d5c1f3434f",
            "1d87"
        ),
    );
    check(
        None,
        &[0x0b; 22],
        b"",
        concat!(
            "8da4e775a563c18f715f802a063c5a31",
            "b8a11f5c5ee1879ec3454e5f3c738d2d",
            "9d201395faa4b61a96c8",
        ),
    );
}

#[test]
fn nist_sp800_38a_aes128_cbc_and_pkcs7() {
    let key = SessionKey(Zeroizing::new(
        bytes("2b7e151628aed2a6abf7158809cf4f3c")
            .try_into()
            .unwrap(),
    ));
    let iv = bytes("000102030405060708090a0b0c0d0e0f");
    let ciphertext = bytes(concat!(
        "7649abac8119b246cee98e9b12e9197d",
        "5086cb9b507219ee95db113a917678b2",
        "73bed6b8e3c1743b7116e69e22229516",
        "3ff1caa1681fac09120eca307586e1a7",
        // Independently computed with OpenSSL as AES-CBC of one block
        // of 0x10 with the fourth NIST ciphertext block as its IV.
        "8cb82807230e1321d3fae00d18cc2012",
    ));
    let expected = bytes(concat!(
        "6bc1bee22e409f96e93d7e117393172a",
        "ae2d8a571e03ac9c9eb76fac45af8e51",
        "30c81c46a35ce411e5fbc1191a0a52ef",
        "f69f2445df4f9b17ad2b417be66c3710",
    ));
    assert_eq!(
        key.encrypt_with_iv(iv.as_slice().try_into().unwrap(), &expected)
            .unwrap(),
        ciphertext
    );
    let decrypted = key
        .decrypt(&iv, ciphertext.clone())
        .expect("known padded ciphertext");
    assert_eq!(decrypted.as_slice(), expected);

    let mut tampered = ciphertext;
    tampered[63] ^= 0x10; // Flip the last padding byte to zero through CBC.
    assert!(matches!(
        key.decrypt(&iv, tampered),
        Err(CryptoError::InvalidCiphertext)
    ));
    assert!(matches!(
        key.decrypt(&iv[..15], vec![0; 16]),
        Err(CryptoError::InvalidCiphertext)
    ));
    for size in [0, 15, 17] {
        assert!(matches!(
            key.decrypt(&iv, vec![0; size]),
            Err(CryptoError::InvalidCiphertext)
        ));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn encryption_draws_a_fresh_iv_for_each_secret() {
    let key = exchange(2).finish(&[4]).unwrap();
    let secret = b"credential canary pear";
    let (first_iv, first) = key.encrypt(secret).unwrap();
    let (second_iv, second) = key.encrypt(secret).unwrap();
    assert_ne!(first_iv, second_iv);
    assert_ne!(first, second);
    assert!(key.decrypt(&first_iv, first).unwrap().as_slice() == secret);
    assert!(key.decrypt(&second_iv, second).unwrap().as_slice() == secret);
}

#[test]
fn session_keeps_the_leading_zero_in_public_key() {
    let client = exchange(1015);
    let mut expected_public = [0; PUBLIC_BYTES];
    expected_public[1] = 0x80; // 2^1015
    assert_eq!(client.public_key(), expected_public);
    // Independently computed as HKDF-SHA256 of pow(4, 1015, p), encoded
    // as a 128-byte unsigned integer, with 32 zero salt bytes and empty info.
    let expected_key = bytes("07fcdb708fc5121d1d80960e512812af");
    assert_eq!(client.finish(&[4]).unwrap().0.as_slice(), expected_key);
    assert_eq!(
        exchange(2).finish(&expected_public).unwrap().0.as_slice(),
        expected_key
    );
}

#[test]
fn session_keeps_the_leading_zero_in_the_shared_secret() {
    // Python pow(4, 1270, p), using the RFC 2409 prime, starts with 00 37 35 42.
    // Python hmac yields this key for 128 bytes and a different key for 127.
    let expected = [
        0xcb, 0xd5, 0x2a, 0x69, 0xc0, 0x84, 0x4a, 0xf8, 0x1f, 0x9d, 0x0e, 0x23, 0x6c, 0xc9, 0x69,
        0xde,
    ];
    assert_eq!(exchange(1270).finish(&[4]).unwrap().0.as_slice(), expected);
}

#[test]
fn accepts_leading_zero_extension_of_a_peer_key() {
    let mut extended = [0; PUBLIC_BYTES + 1];
    extended[PUBLIC_BYTES] = 4;
    let extended_key = exchange(1270).finish(&extended).unwrap();
    let group_width_key = exchange(1270).finish(&extended[1..]).unwrap();
    assert_eq!(extended_key.0.as_slice(), group_width_key.0.as_slice());

    extended[0] = 1;
    assert!(matches!(
        exchange(1270).finish(&extended),
        Err(CryptoError::InvalidPeerKey)
    ));
}

#[test]
fn rejects_invalid_peer_keys() {
    let below_prime = PRIME.wrapping_sub(&U1024::ONE).to_be_bytes();
    let prime = PRIME.to_be_bytes();
    let above_prime = PRIME.wrapping_add(&U1024::ONE).to_be_bytes();
    for invalid in [
        &[][..],
        &[0],
        &[1],
        below_prime.as_ref(),
        prime.as_ref(),
        above_prime.as_ref(),
        &[0; PUBLIC_BYTES + 1],
        // 5 is a non-residue in the group.
        &[5],
    ] {
        assert!(matches!(
            exchange(2).finish(invalid),
            Err(CryptoError::InvalidPeerKey)
        ));
    }
}
