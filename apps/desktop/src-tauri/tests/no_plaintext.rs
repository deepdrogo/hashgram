//! Nothing plaintext (keys, mnemonic, mail, files) is written outside the
//! encrypted vault, the SDK's sealed local store and the sealed UI-cache
//! columns. This test creates an account and private data exactly as the
//! app does — a vault, a mail record in the SDK store, a Drive manifest
//! with a file, a draft, settings, a sealed UI value — then scans every
//! byte of every file under the data dir for the secrets.
//!
//! No network: the SDK store and Drive manifest are written directly
//! through the same public SDK types the facade uses.

// A panic in this test is a failed test, not a crash in the app.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::path::Path;

use hashgram_desktop_lib::crypto;
use hashgram_desktop_lib::db::Db;
use hashgram_sdk::account::Account;
use hashgram_sdk::mail::{DraftRecord, MailRecord};
use hashgram_sdk::protocol::drive as d;
use hashgram_sdk::protocol::pb as app;
use hashgram_sdk::store::LocalStore;
use hashgram_sdk::KdfCost;

const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
const SUBJECT: &str = "Q3 board minutes, strictly confidential";
const BODY: &str = "meet me at the old lighthouse at midnight";
const FILE_NAME: &str = "lighthouse-blueprint.pdf";
const FILE_CONTENT: &[u8] = b"PDF-ISH bytes: the plan for the lighthouse restoration budget";
const DRAFT_BODY: &str = "unsent thoughts about the merger";

fn files_under(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                files_under(&p, out);
            } else {
                out.push(p);
            }
        }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn no_secret_is_ever_written_in_the_clear() {
    let home = std::env::temp_dir().join(format!("hg-plaintext-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let paths = hashgram_sdk::Paths::new(&home);

    // 1. The vault, as onboarding writes it (keystore.json).
    let mut account = Account::import(
        &paths.vault(),
        "correct horse battery staple",
        MNEMONIC,
        "test-pc",
        KdfCost::light(),
    )
    .unwrap();
    let wallet_secret_hex = hex::encode(account.wallet().unwrap().secret_bytes());
    let db_key = crypto::generate_key().unwrap();
    account
        .contents
        .extra
        .insert("desktop_db_key".to_owned(), crypto::key_to_hex(&db_key));
    let device_seed = account.device_seed().unwrap();
    let address = account.address().to_owned();

    // 2. The SDK's local store: a mail record and a draft, as the facade
    //    files them.
    let store = LocalStore::open(&paths.store(), &device_seed).unwrap();
    let msg = app::MailMessage {
        version: 1,
        message_id: vec![7; 16],
        thread_id: vec![7; 16],
        from: Some(app::MailAddress {
            address: "hash1sender".into(),
            username: "sender".into(),
            display_name: String::new(),
        }),
        to: vec![app::MailAddress {
            address: address.clone(),
            ..Default::default()
        }],
        created_at_ms: 1,
        subject: SUBJECT.into(),
        body_text: BODY.into(),
        ..Default::default()
    };
    let rec = MailRecord {
        message: msg,
        folder: "inbox".into(),
        read: false,
        starred: false,
        labels: vec![],
        received_at_ms: 2,
        authenticated_sender: "hash1sender".into(),
        group_id: "aa".into(),
        delivered_to: BTreeMap::new(),
        read_by: BTreeMap::new(),
        outgoing: false,
        trust_score: 0,
    };
    store.put("mail/msg", &[7u8; 16], &rec).unwrap();
    let draft = DraftRecord {
        to: vec!["@bob".into()],
        subject: "draft subject".into(),
        body_text: DRAFT_BODY.into(),
        ..Default::default()
    };
    store.put("mail/draft", b"d1", &draft).unwrap();

    // 3. A Drive manifest with a sealed file, as `Drive::upload` builds it,
    //    persisted locally (the ciphertext of the file too, as a cache
    //    would hold it).
    let device_pk = account.device().unwrap().public_key().to_vec();
    let mut manifest = d::Manifest::new(&device_pk).unwrap();
    let (ct, r) = d::seal_object(FILE_CONTENT).unwrap();
    let object_key = r.key.clone().unwrap().key;
    manifest
        .add_file(&[], FILE_NAME, "application/pdf", r, &device_pk)
        .unwrap();
    store
        .put_bytes("drive/manifest", b"current", &manifest.encode())
        .unwrap();
    std::fs::create_dir_all(paths.cache()).unwrap();
    std::fs::write(paths.cache().join("blob.bin"), &ct).unwrap();
    // The manifest key travels in the vault, sealed with it.
    let manifest_key_hex = hex::encode([0x42u8; 32]);
    account
        .contents
        .extra
        .insert(
            hashgram_sdk::drive::VAULT_DRIVE_KEYRING.to_owned(),
            serde_json::to_string(&serde_json::json!({
                "drive_id": hex::encode([1u8; 16]),
                "key": manifest_key_hex,
                "base_nonce": hex::encode([2u8; 24]),
                "manifest_cid": "",
                "manifest_ref": null,
                "revision": 1
            }))
            .unwrap(),
        );
    account.save().unwrap();
    drop(store);

    // 4. The UI cache with a sealed value, and settings.
    let db = Db::open(&home.join("ui-cache.db")).unwrap();
    db.sealed_put(&db_key, "mail/order", format!("{{\"last\":\"{SUBJECT}\"}}").as_bytes())
        .unwrap();
    db.pending_put("ABCD", "Send 1 HASH to hash1…", "pending").unwrap();
    db.search_note("@alice").unwrap();
    drop(db);
    let settings = hashgram_desktop_lib::settings::Settings::default();
    settings.save(&home.join("settings.json")).unwrap();

    // 5. Scan every file.
    let mut files = Vec::new();
    files_under(&home, &mut files);
    assert!(files.len() >= 4, "expected vault, store, db and settings; got {files:?}");
    let needles: Vec<(&str, Vec<u8>)> = vec![
        ("mnemonic", MNEMONIC.as_bytes().to_vec()),
        ("mnemonic tail", b"abandon abandon art".to_vec()),
        ("wallet secret hex", wallet_secret_hex.as_bytes().to_vec()),
        ("wallet secret raw", account.wallet().unwrap().secret_bytes().to_vec()),
        ("db key hex", crypto::key_to_hex(&db_key).into_bytes()),
        ("db key raw", db_key.as_slice().to_vec()),
        ("root seed", account.root().unwrap().secret_bytes().to_vec()),
        ("device seed", device_seed.to_vec()),
        ("device seed hex", hex::encode(device_seed).into_bytes()),
        ("drive object key", object_key.clone()),
        ("drive object key hex", hex::encode(&object_key).into_bytes()),
        ("manifest key hex", manifest_key_hex.into_bytes()),
        ("mail subject", SUBJECT.as_bytes().to_vec()),
        ("mail body", BODY.as_bytes().to_vec()),
        ("draft body", DRAFT_BODY.as_bytes().to_vec()),
        ("file name", FILE_NAME.as_bytes().to_vec()),
        ("file content", FILE_CONTENT.to_vec()),
        ("file content head", FILE_CONTENT[..20].to_vec()),
    ];
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        for (what, needle) in &needles {
            assert!(!contains(&bytes, needle), "{what} found in the clear in {}", f.display());
        }
    }
    assert!(std::fs::metadata(paths.vault()).unwrap().len() > 200);
    assert!(paths.store().exists());
    let _ = std::fs::remove_dir_all(&home);
}
