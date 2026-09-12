//! Nothing plaintext (keys, mnemonic, messages) is written outside the
//! encrypted vault and the sealed database columns. This test creates an
//! account and some private data exactly as the app does, then scans every
//! byte of every file the app wrote for the secrets.

// A panic in this test is a failed test, not a crash in the app.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;

use hashgram_desktop_lib::crypto;
use hashgram_desktop_lib::db::Db;
use hashgram_sdk::account::Account;
use hashgram_sdk::KdfCost;

const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
const SECRET_MESSAGE: &str = "meet me at the old lighthouse at midnight";
const SECRET_NOTE: &str = "this contact is my accountant";

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

    // 1. The vault, as onboarding writes it.
    let vault = home.join("vault.json");
    let mut account = Account::import(
        &vault,
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
    account.save().unwrap();
    let address = account.address().to_owned();

    // 2. The database with a private contact note and a sealed message.
    let db = Db::open(&home.join("hashgram.db")).unwrap();
    db.contact_put(
        &db_key,
        &address,
        format!("{{\"note\":\"{SECRET_NOTE}\"}}").as_bytes(),
    )
    .unwrap();
    let sealed = crypto::seal(&db_key, b"msg-1", SECRET_MESSAGE.as_bytes()).unwrap();
    db.with(|c| {
        c.execute(
            "INSERT INTO messages(id, group_id, sender, device, ts, kind, sealed) VALUES('msg-1','g','hash1x','d',1,'text',?1)",
            [sealed],
        )
        .map(|_| ())
    })
    .unwrap();
    db.pending_put("ABCD", "Send 1 HASH to hash1…", "pending")
        .unwrap();
    db.search_note("@alice").unwrap();
    drop(db);

    // 3. Settings, as the app writes them.
    let settings = hashgram_desktop_lib::settings::Settings::default();
    settings.save(&home.join("settings.json")).unwrap();

    // 4. Scan every file.
    let mut files = Vec::new();
    files_under(&home, &mut files);
    assert!(
        files.len() >= 3,
        "expected vault, db and settings; got {files:?}"
    );
    let needles: Vec<(&str, Vec<u8>)> = vec![
        ("mnemonic", MNEMONIC.as_bytes().to_vec()),
        ("mnemonic tail", b"abandon abandon art".to_vec()),
        ("wallet secret hex", wallet_secret_hex.as_bytes().to_vec()),
        (
            "wallet secret raw",
            account.wallet().unwrap().secret_bytes().to_vec(),
        ),
        ("db key hex", crypto::key_to_hex(&db_key).into_bytes()),
        ("db key raw", db_key.as_slice().to_vec()),
        ("message", SECRET_MESSAGE.as_bytes().to_vec()),
        ("contact note", SECRET_NOTE.as_bytes().to_vec()),
        ("root seed", account.root().unwrap().secret_bytes().to_vec()),
        (
            "device seed",
            account.device().unwrap().secret_bytes().to_vec(),
        ),
    ];
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        for (what, needle) in &needles {
            assert!(
                !contains(&bytes, needle),
                "{what} found in the clear in {}",
                f.display()
            );
        }
    }
    // Sanity: the public address may appear (it is public), and the vault
    // is not empty.
    assert!(std::fs::metadata(&vault).unwrap().len() > 200);
    let _ = std::fs::remove_dir_all(&home);
}
