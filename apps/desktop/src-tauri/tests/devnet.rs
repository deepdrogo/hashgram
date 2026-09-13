//! Headless integration test on a local devnet: the same setup as
//! `scripts/testnet/hashgram-one-e2e.sh` (mock chain gateway + one
//! store/relay node), two `HashgramOne` backends built exactly as the
//! desktop's session builds them, driven through the SDK calls the
//! command modules wrap, with the results mapped through the desktop's
//! views. Asserts: send mail A→B with an inline and a live Drive
//! attachment → Requests on B → accept → reply lands in A's Inbox with a
//! delivery receipt; Drive share visible on B; Space created by A, B
//! invited as Member, a Guest may not post; the views carry no key.
//!
//! Needs `node/target/debug/{mock-gateway,hashgram-node}.exe`; skipped
//! (with a message) when they are missing, so `cargo test` stays green on
//! a machine that did not build the devnet binaries.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use hashgram_desktop_lib::error::UiError;
use hashgram_desktop_lib::state::AppState;
use hashgram_desktop_lib::views::{MailView, SharedWithMeView, SpaceStateView};
use hashgram_sdk::account::Account;
use hashgram_sdk::protocol::mail::Draft;
use hashgram_sdk::protocol::pb as app;
use hashgram_sdk::{Config, HashgramOne, KdfCost, NetworkIdentity, Paths};

const GENESIS: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const GW_PORT: u16 = 31427;
const NODE_PORT: u16 = 26890;
const API_PORT: u16 = 26892;

fn target_dir() -> PathBuf {
    // apps/desktop/src-tauri → node/target/debug
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../node/target/debug")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("../../../node/target/debug"))
}

fn exe(name: &str) -> PathBuf {
    let mut p = target_dir().join(name);
    if cfg!(windows) {
        p.set_extension("exe");
    }
    p
}

struct Devnet {
    gateway: Child,
    node: Child,
    dir: PathBuf,
    peer: String,
}

impl Drop for Devnet {
    fn drop(&mut self) {
        let _ = self.node.kill();
        let _ = self.gateway.kill();
        let _ = self.node.wait();
        let _ = self.gateway.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn wait_http(url: &str, tries: u32) -> bool {
    let c = reqwest::Client::new();
    for _ in 0..tries {
        if c.get(url).send().await.map(|r| r.status().is_success()).unwrap_or(false) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

async fn start_devnet() -> Option<Devnet> {
    let gw = exe("mock-gateway");
    let nd = exe("hashgram-node");
    if !gw.exists() || !nd.exists() {
        eprintln!("devnet binaries missing ({} / {}); skipping", gw.display(), nd.display());
        return None;
    }
    let dir = std::env::temp_dir().join(format!("hg-desktop-devnet-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("node/data")).unwrap();
    let gateway = Command::new(&gw)
        .args(["--port", &GW_PORT.to_string(), "--block-secs", "2"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("mock-gateway");
    assert!(
        wait_http(&format!("http://127.0.0.1:{GW_PORT}/cosmos/base/tendermint/v1beta1/node_info"), 40).await,
        "gateway did not start"
    );
    let home = dir.join("node/data").display().to_string().replace('\\', "/");
    std::fs::write(
        dir.join("node/network.json"),
        format!("{{\"network_id\":\"hashgram-devnet\",\"genesis_hash\":\"{GENESIS}\"}}"),
    )
    .unwrap();
    std::fs::write(
        dir.join("node/roles.json"),
        "{\"roles\":[\"relay\",\"store\",\"media\",\"bootstrap\"],\"reward_address\":\"\",\"declared_storage_bytes\":1000000000}",
    )
    .unwrap();
    std::fs::write(
        dir.join("node/node.toml"),
        format!(
            "listen_addr = \"127.0.0.1\"\nlisten_port = {NODE_PORT}\ntransport = \"tcp\"\napi_addr = \"127.0.0.1:{API_PORT}\"\nmetrics_addr = \"127.0.0.1:{}\"\nchain_rpc = \"http://127.0.0.1:{}\"\nchain_api = \"http://127.0.0.1:{GW_PORT}\"\npeerstore_path = \"{home}/peerstore.json\"\ndata_dir = \"{home}\"\nbootstrap_peers = []\nmin_peers = 0\nstorage_quota_bytes = 1000000000\n",
            API_PORT - 1,
            GW_PORT - 1
        ),
    )
    .unwrap();
    let node = Command::new(&nd)
        .args([
            "run",
            "--home",
            &dir.join("node/data").display().to_string(),
            "--config",
            &dir.join("node/node.toml").display().to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("hashgram-node");
    assert!(wait_http(&format!("http://127.0.0.1:{API_PORT}/v1/status"), 80).await, "node did not start");
    let status: serde_json::Value = reqwest::get(format!("http://127.0.0.1:{API_PORT}/v1/status"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let peer = status["swarm"]["peer_id"].as_str().unwrap().to_owned();
    Some(Devnet {
        gateway,
        node,
        dir,
        peer,
    })
}

fn config(home: &Path, peer: &str) -> Config {
    Config {
        paths: Paths::new(home),
        network: NetworkIdentity::devnet(GENESIS),
        bootstrap: vec![format!("/ip4/127.0.0.1/tcp/{NODE_PORT}/p2p/{peer}").parse().unwrap()],
        chain_api: Some(format!("http://127.0.0.1:{GW_PORT}")),
        kdf: KdfCost::light(),
        connect_wait: Duration::from_secs(10),
    }
}

async fn register(one: &mut HashgramOne, username: &str) {
    let (did, pk) = one.devices().this_device().unwrap();
    let body = serde_json::json!({
        "address": one.address(),
        "username": username,
        "devices": [{"device_id": did, "device_pubkey": pk, "label": "test", "platform": "windows"}],
    });
    let r = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{GW_PORT}/devnet/identity"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let n = one
        .messaging
        .publish_key_package(&one.link, &one.network)
        .await
        .unwrap();
    assert!(n > 0, "key packages published to {n} stores");
    one.save().unwrap();
}

fn forbidden(json: &str) {
    let j = json.to_ascii_lowercase();
    for word in ["\"key\":", "\"nonce\"", "\"seed\"", "\"secret\"", "\"mnemonic\"", "base_nonce", "manifest_key"] {
        assert!(!j.contains(word), "{word} leaked into a view: {json}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_backends_exchange_mail_drive_and_space_through_views() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")))
        .with_test_writer()
        .try_init();
    let Some(net) = start_devnet().await else { return };
    let peer = net.peer.clone();
    let alice_home = net.dir.join("alice");
    let bob_home = net.dir.join("bob");
    std::fs::create_dir_all(&alice_home).unwrap();
    std::fs::create_dir_all(&bob_home).unwrap();

    // Accounts exactly as onboarding creates them.
    let (a_acct, _m) = Account::create(&Paths::new(&alice_home).vault(), "alice-pass-1234", "alice-1", KdfCost::light()).unwrap();
    let (b_acct, _m) = Account::create(&Paths::new(&bob_home).vault(), "bob-pass-12345", "bob-1", KdfCost::light()).unwrap();

    // The desktop's session: one link per process, facade over it.
    let a_link = HashgramOne::connect_link(&config(&alice_home, &peer)).await.unwrap();
    let mut alice = HashgramOne::with_link(config(&alice_home, &peer), a_acct, a_link).await.unwrap();
    let b_link = HashgramOne::connect_link(&config(&bob_home, &peer)).await.unwrap();
    let mut bob = HashgramOne::with_link(config(&bob_home, &peer), b_acct, b_link).await.unwrap();
    assert!(!alice.link.peers().await.is_empty(), "alice sees the node");
    assert!(!bob.link.peers().await.is_empty(), "bob sees the node");
    register(&mut alice, "alice").await;
    register(&mut bob, "bob").await;

    // People: resolve @bob and bob@hashgram.io.
    let r = alice.people().resolve("@bob").await.unwrap();
    assert_eq!(r.username, "bob");
    assert_eq!(r.address, bob.address());
    assert!(r.has_identity);
    let r2 = alice.people().resolve("bob@hashgram.io").await.unwrap();
    assert_eq!(r2.address, bob.address());

    // Drive: upload a file (uncommitted → commit).
    let entry = alice
        .drive()
        .upload("", "contract.txt", "text/plain", b"contract v1")
        .await
        .unwrap();
    assert!(alice.drive().usage().dirty);
    alice.drive().commit().await.unwrap();
    assert!(!alice.drive().usage().dirty);

    // Mail A→B with an inline and a live Drive attachment (as mail_send does).
    let to = alice.mail().resolve_recipients(&["@bob".to_owned()]).await.unwrap();
    let inline = alice
        .mail()
        .make_attachment("note.txt", "text/plain", b"inline note")
        .await
        .unwrap();
    let live = alice
        .mail()
        .attach_from_drive(&entry, &[bob.address().to_owned()], true)
        .await
        .unwrap();
    let id = alice
        .mail()
        .send(Draft {
            to,
            subject: "Contract for review".into(),
            body_text: "see attached".into(),
            attachments: vec![inline, live],
            ..Default::default()
        })
        .await
        .unwrap();
    alice.save().unwrap();

    // Bob syncs: mail in Requests (stranger) + the Drive share.
    let rep = bob.sync().round().await.unwrap();
    assert_eq!(rep.mail, 1, "bob received one mail: {rep:?}");
    assert!(rep.drive >= 1, "bob received the live share: {rep:?}");
    let counts = bob.mail().counts();
    assert_eq!(counts["requests"].total, 1);
    assert_eq!(counts["inbox"].total, 0);
    let rec = bob.mail().get(&id).unwrap().unwrap();
    let view = MailView::from(&rec);
    let json = serde_json::to_string(&view).unwrap();
    forbidden(&json);
    assert_eq!(view.authenticated_sender, alice.address());
    assert!(view.sender_matches);
    assert_eq!(view.attachments.len(), 2);
    assert_eq!(view.attachments[0].kind, "inline");
    assert_eq!(view.attachments[1].kind, "drive");
    assert!(view.attachments[1].live);
    // Attachments decrypt on the Rust side only.
    let a0 = bob.mail().attachment_bytes(&rec.message.attachments[0]).await.unwrap();
    assert_eq!(a0, b"inline note");
    let a1 = bob.mail().attachment_bytes(&rec.message.attachments[1]).await.unwrap();
    assert_eq!(a1, b"contract v1");
    let shared: Vec<SharedWithMeView> = bob.drive().shared_with_me().unwrap().iter().map(SharedWithMeView::from).collect();
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].capability.mode, "live");
    forbidden(&serde_json::to_string(&shared).unwrap());

    // Requests → Accept, reply → lands in Alice's Inbox with a delivery receipt.
    bob.mail().accept_request(&id).unwrap();
    assert_eq!(bob.mail().counts()["inbox"].total, 1);
    let reply = bob.mail().reply_draft(&id, false).unwrap();
    bob.mail()
        .send(Draft {
            body_text: "Looks good".into(),
            ..reply
        })
        .await
        .unwrap();
    bob.save().unwrap();
    let rep = alice.sync().round().await.unwrap();
    assert_eq!(rep.mail, 1, "alice receives the reply: {rep:?}");
    let inbox = alice.mail().list("inbox", 0, 10).unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].subject, "Re: Contract for review");
    let sent = alice.mail().get(&id).unwrap().unwrap();
    assert!(sent.delivered_to.contains_key(bob.address()), "delivery receipt: {:?}", sent.delivered_to);
    let thread = alice.mail().thread(&inbox[0].thread_id).unwrap().unwrap();
    assert_eq!(thread.messages.len(), 2);

    // Live share update reaches Bob as a newer version.
    alice.drive().update(&entry, b"contract v2", "").await.unwrap();
    alice.save().unwrap();
    let mut shared = bob.drive().shared_with_me().unwrap();
    for _ in 0..12 {
        bob.sync().round().await.unwrap();
        shared = bob.drive().shared_with_me().unwrap();
        if shared[0].capability.version_no == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(shared[0].capability.version_no, 2);
    assert_eq!(shared[0].updates, 1);

    // Space: Alice creates, invites Bob as Guest; a Guest may not post.
    let space = alice.spaces().create("Project X", "the deal").await.unwrap();
    alice.spaces().invite(&space, bob.address(), app::SpaceRole::Guest).await.unwrap();
    alice.spaces().announce(&space, "Kick-off", "Monday", Vec::new()).await.unwrap();
    alice.save().unwrap();
    for _ in 0..12 {
        bob.sync().round().await.unwrap();
        if bob.spaces().list().map(|l| !l.is_empty()).unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let list = bob.spaces().list().unwrap();
    assert_eq!(list.len(), 1, "bob sees the space");
    assert_eq!(list[0].my_role, app::SpaceRole::Guest as i32);
    let st = bob.spaces().state(&space).unwrap();
    let sv = SpaceStateView::of(&st, bob.address());
    forbidden(&serde_json::to_string(&sv).unwrap());
    assert_eq!(sv.members.len(), 2);
    let err = match bob.spaces().post(&space, "hi", Vec::new(), Vec::new()).await {
        Ok(_) => panic!("a guest must not be able to post"),
        Err(e) => e,
    };
    let ui = UiError::from(err);
    assert_eq!(ui.code, "invalid");
    assert!(ui.message.contains("space rule"), "{}", ui.message);
    // Promote to Member → posting works.
    alice.spaces().set_role(&space, bob.address(), app::SpaceRole::Member).await.unwrap();
    alice.save().unwrap();
    for _ in 0..12 {
        bob.sync().round().await.unwrap();
        if bob.spaces().state(&space).map(|s| s.role_of(bob.address()) == app::SpaceRole::Member).unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bob.spaces().post(&space, "hello from bob", Vec::new(), Vec::new()).await.unwrap();
    bob.save().unwrap();
    alice.sync().round().await.unwrap();
    let content = alice.spaces().content(&space, 0, 10).unwrap();
    assert!(content.iter().any(|c| c.kind == "post" && c.actor == bob.address()));

    // Lock semantics: a dropped facade means every command answers `locked`.
    let mut slot: Option<HashgramOne> = Some(alice);
    assert!(AppState::unlocked(&mut slot).is_ok());
    slot = None;
    let e = match AppState::unlocked(&mut slot) {
        Ok(_) => panic!("a dropped facade must read as locked"),
        Err(e) => e,
    };
    assert_eq!(e.code, "locked");
    drop(bob);
}
