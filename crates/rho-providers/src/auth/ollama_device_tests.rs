use super::*;
use tempfile::tempdir;

#[test]
fn loads_ollama_generated_pem_wrapped_at_64_columns() {
    let private_key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    let pem = private_key.to_openssh(LineEnding::LF).unwrap();
    let payload = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<String>();
    let mut ollama_pem = format!("{OPENSSH_PEM_BEGIN}\n");
    for chunk in payload.as_bytes().chunks(64) {
        ollama_pem.push_str(std::str::from_utf8(chunk).unwrap());
        ollama_pem.push('\n');
    }
    ollama_pem.push_str(OPENSSH_PEM_END);

    let key = OllamaDeviceKey::from_openssh_private_key(&ollama_pem).unwrap();
    assert_eq!(
        private_key.public_key().to_openssh().unwrap(),
        key.public_key_openssh()
    );
    key.sign_authorization("POST,/api/me?ts=1700000000")
        .unwrap();
}

#[test]
fn generates_and_reloads_device_key() {
    let dir = tempdir().unwrap();
    let created = OllamaDeviceKey::load_or_create(dir.path()).unwrap();
    let loaded = OllamaDeviceKey::load_from_dir(dir.path()).unwrap();
    assert_eq!(created.public_key_openssh(), loaded.public_key_openssh());
    assert!(dir.path().join(DEFAULT_PRIVATE_KEY_NAME).exists());
    assert!(dir.path().join(DEFAULT_PUBLIC_KEY_NAME).exists());
}

#[test]
fn load_or_create_does_not_truncate_existing_private_key() {
    let dir = tempdir().unwrap();
    let first = OllamaDeviceKey::load_or_create(dir.path()).unwrap();
    let private_path = dir.path().join(DEFAULT_PRIVATE_KEY_NAME);
    let before = std::fs::read_to_string(&private_path).unwrap();
    let second = OllamaDeviceKey::load_or_create(dir.path()).unwrap();
    let after = std::fs::read_to_string(&private_path).unwrap();
    assert_eq!(first.public_key_openssh(), second.public_key_openssh());
    assert_eq!(before, after);
}

#[test]
fn create_new_reports_already_exists_without_truncating() {
    let dir = tempdir().unwrap();
    let first = create_key_pair(dir.path()).unwrap();
    let before = std::fs::read_to_string(dir.path().join(DEFAULT_PRIVATE_KEY_NAME)).unwrap();
    let err = create_key_pair(dir.path()).unwrap_err();
    assert!(matches!(err, OllamaDeviceError::AlreadyExists));
    let after = std::fs::read_to_string(dir.path().join(DEFAULT_PRIVATE_KEY_NAME)).unwrap();
    assert_eq!(before, after);
    let loaded = OllamaDeviceKey::load_from_dir(dir.path()).unwrap();
    assert_eq!(first.public_key_openssh(), loaded.public_key_openssh());
}

#[test]
fn signs_authorization_in_ollama_format() {
    let key = generate_key().unwrap();
    let authorization = key
        .sign_authorization("POST,/api/me?ts=1700000000")
        .unwrap();
    let (pubkey, signature) = authorization.split_once(':').unwrap();
    assert!(!pubkey.is_empty());
    assert!(!signature.is_empty());
    // Standard base64 payload and signature blobs.
    general_purpose::STANDARD.decode(pubkey).unwrap();
    let sig = general_purpose::STANDARD.decode(signature).unwrap();
    assert_eq!(sig.len(), 64);
}

#[test]
fn authorize_request_adds_ts_and_raw_authorization() {
    let key = generate_key().unwrap();
    let url = url::Url::parse("https://ollama.com/v1/chat/completions").unwrap();
    let (signed_url, authorization) = key.authorize_request("POST", url).unwrap();
    assert!(signed_url.query().unwrap().contains("ts="));
    assert!(authorization.contains(':'));
    assert!(!authorization.starts_with("Bearer "));
}

#[test]
fn connect_url_includes_encoded_public_key() {
    let key = generate_key().unwrap();
    let url = key.connect_url().unwrap();
    assert!(url.starts_with("https://ollama.com/connect?"));
    assert!(url.contains("name="));
    assert!(url.contains("key="));
}
