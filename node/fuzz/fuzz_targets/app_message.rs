#![no_main]
//! Hostile bytes through every Hashgram One application decoder: the
//! envelope gate, Mail validation, Drive manifest/capability validation,
//! Space structural validation and Circle validation. None may panic.
use hashgram_app::{circle, drive, envelope, mail, pb, space};
use libfuzzer_sys::fuzz_target;
use prost::Message;

fuzz_target!(|data: &[u8]| {
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
        if let Ok(a) = drive::Manifest::from_pb(m.clone()) {
            let _ = drive::merge(&a, &a);
            let _ = a.list(&[], false);
        }
    }
    if let Ok(r) = pb::DriveObjectRef::decode(data) {
        let _ = drive::open_object(data, &r);
    }
});
