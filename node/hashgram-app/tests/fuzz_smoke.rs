//! The `app_message` fuzz target as an ordinary test: random and mutated
//! inputs through every application decoder must never panic.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use hashgram_app::{circle, drive, envelope, mail, pb, space};
use prost::Message;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let n = (self.next() as usize) % (max + 1);
        (0..n).map(|_| self.next() as u8).collect()
    }
    fn mutate(&mut self, base: &[u8]) -> Vec<u8> {
        let mut v = base.to_vec();
        for _ in 0..(1 + self.next() % 4) {
            if v.is_empty() {
                v.push(self.next() as u8);
                continue;
            }
            let i = (self.next() as usize) % v.len();
            match self.next() % 3 {
                0 => v[i] = self.next() as u8,
                1 => {
                    v.remove(i);
                }
                _ => v.insert(i, self.next() as u8),
            }
        }
        v
    }
}

fn exercise(data: &[u8]) {
    if let Ok(envelope::Opened::App(app)) = envelope::open(data) {
        let _ = envelope::kind_name(&app);
        match app.body {
            Some(pb::app_message::Body::Mail(m)) => {
                let _ = mail::validate(&m);
                let _ = mail::participant_set(&m);
                let _ = mail::reply_all_recipients(&m, "hash1x");
            }
            Some(pb::app_message::Body::DriveShare(s)) => {
                if let Some(c) = s.capability {
                    let _ = drive::validate_capability(&c);
                }
            }
            Some(pb::app_message::Body::SpaceEvent(e)) => {
                let _ = space::validate(&e);
                let _ = space::canonical_payload(&e);
            }
            Some(pb::app_message::Body::CircleEvent(e)) => {
                let _ = circle::validate(&e);
                let mut t = circle::Timeline::default();
                let _ = t.apply("hash1a", &e);
            }
            _ => {}
        }
    }
    if let Ok(m) = pb::DriveManifest::decode(data) {
        if let Ok(a) = drive::Manifest::from_pb(m) {
            let _ = drive::merge(&a, &a);
            let _ = a.list(&[], false);
        }
    }
    if let Ok(r) = pb::DriveObjectRef::decode(data) {
        let _ = drive::open_object(data, &r);
    }
}

#[test]
fn random_and_mutated_inputs_never_panic() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let receipt = envelope::wrap(pb::app_message::Body::MailReceipt(pb::MailReceipt {
        version: 1,
        message_id: vec![1; 16],
        kind: 1,
        at_ms: 5,
    }))
    .unwrap();
    let mail = envelope::wrap(pb::app_message::Body::Mail(pb::MailMessage {
        version: 1,
        message_id: vec![2; 16],
        thread_id: vec![2; 16],
        from: Some(pb::MailAddress {
            address: "hash1e0tl2hff03hu4g3sawcjqa2p9tc4uh24e4vfl5".into(),
            ..Default::default()
        }),
        to: vec![pb::MailAddress {
            address: "hash178njz3gft77798ssh6lrqxh2hskw5dm68f2uh5".into(),
            ..Default::default()
        }],
        subject: "s".into(),
        body_text: "b".into(),
        ..Default::default()
    }))
    .unwrap();
    let (_, r) = drive::seal_object(b"hello").unwrap();
    let mut man = drive::Manifest::new(&[1; 32]).unwrap();
    let _ = man.add_file(&[], "f", "text/plain", r.clone(), &[1; 32]);
    let seeds: Vec<Vec<u8>> = vec![
        envelope::encode_for_mls(receipt).unwrap(),
        envelope::encode_for_mls(mail).unwrap(),
        man.encode(),
        r.encode_to_vec(),
    ];
    for _ in 0..2000 {
        exercise(&rng.bytes(300));
    }
    for s in &seeds {
        for _ in 0..500 {
            exercise(&rng.mutate(s));
        }
        exercise(s);
    }
}
