use super::*;

#[test]
fn base64_follows_the_standard_alphabet_and_padding() {
    for (plain, coded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64_encode(plain.as_bytes()), coded);
        assert_eq!(&base64_decode(coded).unwrap()[..], plain.as_bytes());
    }
    assert_eq!(&base64_decode("+/8=").unwrap()[..], &[0xfb, 0xff]);
    for bad in ["Z", "Zm9v!", "Zm-v"] {
        assert_eq!(
            base64_decode(bad).err(),
            Some(ExchangeError::Malformed),
            "{bad}"
        );
    }
}

#[test]
fn a_private_key_of_one_has_the_generator_as_its_public_key() {
    let side = Exchange::with_private(&[1]);
    assert_eq!(side.begin(), "[sx-aes-1]\npublic=Ag==\n");
    assert!(!side.shared());
}

#[test]
fn two_sides_share_a_key_and_a_secret_crosses_in_either_direction() {
    let mut prompter = Exchange::new().unwrap();
    let mut caller = Exchange::new().unwrap();
    assert_eq!(caller.receive(&prompter.begin()).unwrap(), None);
    assert_eq!(prompter.receive(&caller.begin()).unwrap(), None);
    assert!(prompter.shared() && caller.shared());

    let sent = prompter.send(Some(b"correct horse")).unwrap();
    assert!(sent.starts_with("[sx-aes-1]\npublic="));
    assert!(sent.contains("\nsecret=") && sent.contains("\niv="));
    assert!(!sent.contains("correct horse"));
    assert_eq!(
        caller.receive(&sent).unwrap().as_deref().map(Vec::as_slice),
        Some(&b"correct horse"[..])
    );
    let back = caller.send(Some("pässwörd".as_bytes())).unwrap();
    assert_eq!(
        prompter
            .receive(&back)
            .unwrap()
            .as_deref()
            .map(Vec::as_slice),
        Some("pässwörd".as_bytes())
    );
    // without a secret a message is the public key alone
    assert_eq!(prompter.send(None).unwrap(), prompter.begin());
}

#[test]
fn each_secret_gets_a_fresh_iv_and_its_length_rounds_up_to_a_block() {
    let mut a = Exchange::new().unwrap();
    let mut b = Exchange::new().unwrap();
    b.receive(&a.begin()).unwrap();
    a.receive(&b.begin()).unwrap();
    let one = read_message(&a.send(Some(b"12345")).unwrap()).unwrap();
    let two = read_message(&a.send(Some(b"12345")).unwrap()).unwrap();
    let value = |m: &[(String, Zeroizing<Vec<u8>>)], k: &str| {
        m.iter().find(|(n, _)| n == k).unwrap().1.to_vec()
    };
    assert_ne!(value(&one, "iv"), value(&two, "iv"));
    assert_ne!(value(&one, "secret"), value(&two, "secret"));
    assert_eq!(value(&one, "iv").len(), 16);
    assert_eq!(value(&one, "secret").len(), 16);
    let full = read_message(&a.send(Some(&[7; 16])).unwrap()).unwrap();
    assert_eq!(
        value(&full, "secret").len(),
        32,
        "a full block gets a block of padding"
    );
}

#[test]
fn a_secret_is_sent_only_after_the_peer_is_known() {
    let a = Exchange::new().unwrap();
    assert_eq!(a.send(Some(b"x")), Err(ExchangeError::NoPeer));
}

#[test]
fn a_public_key_outside_the_group_or_a_broken_message_is_refused() {
    let mut a = Exchange::new().unwrap();
    let prime = PRIME;
    let mut last = prime;
    last[GROUP_BYTES - 1] -= 1;
    for key in [
        vec![0u8],
        vec![1u8],
        last.to_vec(),
        prime.to_vec(),
        vec![1u8; 193],
    ] {
        let message = write_message(&[("public", &key)]);
        assert_eq!(a.receive(&message), Err(ExchangeError::BadPublicKey));
    }
    for message in [
        "",
        "public=Ag==",
        "[other]\npublic=Ag==\n",
        "[sx-aes-1]\npublic=***\n",
        "[sx-aes-1]\nno equals sign\n",
    ] {
        assert!(a.receive(message).is_err(), "{message:?}");
    }
    let b = Exchange::new().unwrap();
    a.receive(&b.begin()).unwrap();
    // a secret without its iv, and one that is not whole blocks
    let half = write_message(&[("public", &b.public), ("secret", &[0; 16])]);
    assert_eq!(a.receive(&half), Err(ExchangeError::Malformed));
    let ragged = write_message(&[
        ("public", &b.public),
        ("secret", &[0; 15]),
        ("iv", &[0; 16]),
    ]);
    assert_eq!(a.receive(&ragged), Err(ExchangeError::Malformed));
}

#[test]
fn a_secret_under_another_key_does_not_decrypt_to_it() {
    let mut a = Exchange::new().unwrap();
    let mut b = Exchange::new().unwrap();
    let mut eve = Exchange::new().unwrap();
    a.receive(&b.begin()).unwrap();
    eve.receive(&b.begin()).unwrap();
    b.receive(&eve.begin()).unwrap();
    let sent = b.send(Some(b"for eve only")).unwrap();
    // the message names b's key, but a holds no key shared with b for eve's
    match a.receive(&sent) {
        Err(ExchangeError::Decrypt) => {}
        Ok(Some(plain)) => assert_ne!(&plain[..], b"for eve only"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn comments_blank_lines_and_other_groups_are_skipped() {
    let mut a = Exchange::new().unwrap();
    let b = Exchange::new().unwrap();
    let message = format!("# a comment\n\n[other]\nx=y\n{}", b.begin());
    assert_eq!(a.receive(&message), Ok(None));
    assert!(a.shared());
}

#[test]
fn a_shared_value_with_a_leading_zero_byte_derives_its_key_from_the_bytes_after_it() {
    // 2^(0x228 * 0x303) mod p begins with the bytes 00 61 24; the key is HKDF-SHA256 of the 191 bytes from 0x61 on,
    // which the keyring daemon derives as well (a key from the 192 padded bytes would be 1bbcbd0b...)
    let mut a = Exchange::with_private(&[0x02, 0x28]);
    let mut b = Exchange::with_private(&[0x03, 0x03]);
    a.receive(&b.begin()).unwrap();
    b.receive(&a.begin()).unwrap();
    let expected = [
        0x8e, 0x83, 0xb9, 0xbe, 0x4c, 0x31, 0x10, 0xdd, 0xe7, 0xa8, 0x99, 0xec, 0x6a, 0x18, 0xb5,
        0x55,
    ];
    assert_eq!(**a.key.as_ref().unwrap(), expected);
    assert_eq!(**b.key.as_ref().unwrap(), expected);
}
