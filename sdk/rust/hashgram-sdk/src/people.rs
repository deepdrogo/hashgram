//! People: identity lookup, contact requests, friends, blocks, profiles.
//!
//! Lookup goes to the chain (`x/username`, `x/identity`) through whatever
//! chain client the facade has (relay or REST). Contact state is the
//! `hashgram_app::people::Contacts` book, persisted in the local store and
//! synced to the user's other devices as `ContactsSnapshot`. Requests and
//! answers travel as application messages in the direct MLS group with the
//! other party.

use hashgram_app::mail::{parse_address, AddressForm};
use hashgram_app::pb as app;
use hashgram_app::people as p;
use tracing::debug;

use crate::app::HashgramOne;
use crate::messaging::Received;
use crate::store::LocalStore;
use crate::SdkError;

const NS: &str = "people";

/// A resolved identity.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Resolved {
    /// Address.
    pub address: String,
    /// Username, if any.
    pub username: String,
    /// Display name (from the public profile or contact book).
    pub display_name: String,
    /// Mail address form (`user@hashgram.io`) when a username exists.
    pub mail_address: String,
    /// Whether the address has an on-chain identity (devices).
    pub has_identity: bool,
    /// Active device count.
    pub devices: usize,
}

/// A public profile as the network shows it.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Profile {
    /// Address.
    pub address: String,
    /// Username.
    pub username: String,
    /// Display name.
    pub display_name: String,
    /// Bio.
    pub bio: String,
    /// Avatar CID (hex).
    pub avatar_cid: String,
    /// Our contact flags for them.
    pub states: Vec<String>,
}

/// A profile with the time it was fetched, for the local cache.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedProfile {
    profile: Profile,
    fetched_at_ms: u64,
}

/// People state.
#[derive(Debug, Default)]
pub struct PeopleState {
    /// The contact book.
    pub contacts: p::Contacts,
    /// Our display name for outgoing mail / cards.
    pub my_display_name: String,
    /// Username cache: address → (username, fetched_at_ms).
    cache: std::collections::HashMap<String, (String, u64)>,
    /// Pending snapshot flag.
    dirty: bool,
}

impl PeopleState {
    pub(crate) fn load(store: &LocalStore) -> Result<Self, SdkError> {
        Ok(Self {
            contacts: store.get(NS, b"contacts")?.unwrap_or_default(),
            my_display_name: store.get(NS, b"my_display_name")?.unwrap_or_default(),
            cache: Default::default(),
            dirty: false,
        })
    }
    pub(crate) fn ensure_self(&mut self, _me: &str) {}
    fn save(&mut self, store: &LocalStore) -> Result<(), SdkError> {
        store.put(NS, b"contacts", &self.contacts)?;
        store.put(NS, b"my_display_name", &self.my_display_name)?;
        self.dirty = true;
        Ok(())
    }
}

/// The People API.
pub struct People<'a> {
    pub(crate) one: &'a mut HashgramOne,
}

impl<'a> People<'a> {
    /// Username registered by an address, if any (cached 10 minutes).
    pub async fn username_of(&mut self, address: &str) -> Result<String, SdkError> {
        let now = hashgram_app::ids::now_ms();
        if let Some((u, at)) = self.one.people_state.cache.get(address) {
            if now.saturating_sub(*at) < 600_000 {
                return Ok(u.clone());
            }
        }
        let v = self
            .one
            .chain
            .query(&format!("hashgram/username/v1/reverse/{address}"))
            .await;
        // `x/username` answers `{registrations: [{name, owner, …}]}`
        // (`proto/hashgram/username/v1/query.proto`); older mocks used
        // `names`, accepted as a fallback.
        let name = match v {
            Ok(v) => v
                .get("registrations")
                .or_else(|| v.get("names"))
                .and_then(|n| n.as_array())
                .and_then(|a| a.first())
                .and_then(|x| x.get("name").or(Some(x)))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned(),
            Err(e) => {
                debug!(error = %e, "reverse username lookup failed");
                String::new()
            }
        };
        self.one
            .people_state
            .cache
            .insert(address.to_owned(), (name.clone(), now));
        Ok(name)
    }

    /// Our own username.
    pub async fn my_username(&mut self) -> Result<String, SdkError> {
        let me = self.one.account.address().to_owned();
        self.username_of(&me).await
    }

    /// Owner of a username.
    pub async fn owner_of(&mut self, username: &str) -> Result<Option<String>, SdkError> {
        let v = self
            .one
            .chain
            .query(&format!("hashgram/username/v1/lookup/{username}"))
            .await;
        match v {
            Ok(v) => {
                // The chain answers 200 with `found: false` for an
                // unregistered name; the registration is then empty.
                if v.get("found").and_then(|f| f.as_bool()) == Some(false) {
                    return Ok(None);
                }
                Ok(v.get("registration")
                    .and_then(|r| r.get("owner"))
                    .or_else(|| v.get("owner"))
                    .and_then(|o| o.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_owned()))
            }
            Err(crate::chain::ClientError::Gateway { status: 404, .. }) => Ok(None),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("not found") || msg.contains("404") {
                    Ok(None)
                } else {
                    Err(e.into())
                }
            }
        }
    }

    /// Resolves anything a user might type: address, `@name`,
    /// `name@hashgram.io`, or bare username.
    pub async fn resolve(&mut self, input: &str) -> Result<Resolved, SdkError> {
        let form = parse_address(input)?;
        let (address, username) = match form {
            AddressForm::Address(a) => {
                let u = self.username_of(&a).await?;
                (a, u)
            }
            AddressForm::Username(u) => {
                let a = self
                    .owner_of(&u)
                    .await?
                    .ok_or_else(|| SdkError::NotFound(format!("no such username: {u}")))?;
                (a, u)
            }
            AddressForm::External(e) => {
                return Err(SdkError::Invalid(format!(
                    "{e} is not a Hashgram identity; external mail goes through a gateway"
                )))
            }
        };
        let devices = crate::account::devices_on_chain(&self.one.chain, &address)
            .await
            .map(|d| d.iter().filter(|x| !x.revoked).count())
            .unwrap_or(0);
        let display_name = self
            .one
            .people_state
            .contacts
            .records
            .get(&address)
            .map(|r| r.display_name.clone())
            .unwrap_or_default();
        Ok(Resolved {
            mail_address: if username.is_empty() {
                String::new()
            } else {
                hashgram_app::mail::mail_address_for(&username)
            },
            address,
            username,
            display_name,
            has_identity: devices > 0,
            devices,
        })
    }

    /// Public profile from the social log (latest `PROFILE_UPDATE`).
    /// Always asks the network; see [`Self::profile_cached`] for lists.
    pub async fn profile(&mut self, address: &str) -> Result<Profile, SdkError> {
        let username = self.username_of(address).await.unwrap_or_default();
        let social = crate::social::Social::open(&self.one.account)?;
        // Newest profile update first; a node that predates the `latest`
        // filter answers with the author's chain from the start, which the
        // fallback below still reads correctly.
        let mut events = social
            .fetch(
                &self.one.link,
                &self.one.network,
                hashgram_proto::pb::EventFetch {
                    author: address.to_owned(),
                    types: vec!["PROFILE_UPDATE".to_owned()],
                    latest: true,
                    limit: 1,
                    ..Default::default()
                },
            )
            .await
            .map(|r| r.events)
            .unwrap_or_default();
        if events.is_empty() {
            events = social
                .fetch_author(&self.one.link, &self.one.network, address, 0, 200)
                .await
                .unwrap_or_default();
        }
        let mut prof = Profile {
            address: address.to_owned(),
            username,
            states: self
                .one
                .people_state
                .contacts
                .records
                .get(address)
                .map(|r| r.states.clone())
                .unwrap_or_default(),
            ..Default::default()
        };
        // Newest PROFILE_UPDATE wins whichever order the node answered in.
        if let Some(ev) = events
            .iter()
            .filter(|e| e.r#type == "PROFILE_UPDATE")
            .max_by_key(|e| (e.timestamp, e.sequence))
        {
            if let Ok(p) = <hashgram_proto::pb::ProfileUpdate as prost::Message>::decode(
                ev.payload.as_slice(),
            ) {
                prof.display_name = p.display_name;
                prof.bio = p.bio;
                prof.avatar_cid = hex::encode(&p.avatar_cid);
            }
        }
        self.one.store.put(
            NS,
            format!("profile/{address}").as_bytes(),
            &CachedProfile {
                profile: prof.clone(),
                fetched_at_ms: hashgram_app::ids::now_ms(),
            },
        )?;
        Ok(prof)
    }

    /// Public profile, from a local cache no older than `max_age_secs`,
    /// otherwise from the network. Cheap enough to call for every author
    /// in a list.
    pub async fn profile_cached(
        &mut self,
        address: &str,
        max_age_secs: u64,
    ) -> Result<Profile, SdkError> {
        let key = format!("profile/{address}");
        if let Some(c) = self.one.store.get::<CachedProfile>(NS, key.as_bytes())? {
            let age_ms = hashgram_app::ids::now_ms().saturating_sub(c.fetched_at_ms);
            if age_ms < max_age_secs.saturating_mul(1000) {
                let mut p = c.profile;
                // Contact flags are local and may have changed since.
                p.states = self
                    .one
                    .people_state
                    .contacts
                    .records
                    .get(address)
                    .map(|r| r.states.clone())
                    .unwrap_or_default();
                return Ok(p);
            }
        }
        self.profile(address).await
    }

    /// Our display name (used in outgoing mail headers and cards).
    #[must_use]
    pub fn my_display_name(&self) -> String {
        self.one.people_state.my_display_name.clone()
    }

    /// Sets our display name (used in outgoing mail headers and cards).
    pub fn set_my_display_name(&mut self, name: &str) -> Result<(), SdkError> {
        if name.len() > 128 {
            return Err(SdkError::Invalid("display name too long".into()));
        }
        self.one.people_state.my_display_name = name.to_owned();
        self.one.people_state.save(&self.one.store)
    }

    // -----------------------------------------------------------------------
    // Requests
    // -----------------------------------------------------------------------

    /// Sends a contact request.
    pub async fn request(&mut self, input: &str, message: &str) -> Result<Resolved, SdkError> {
        let r = self.resolve(input).await?;
        if r.address == self.one.account.address() {
            return Err(SdkError::Invalid("that is you".into()));
        }
        self.one.people_state.contacts.request_sent(&r.address)?;
        let _ = self
            .one
            .people_state
            .contacts
            .set_names(&r.address, &r.username, &r.display_name);
        let gid = self
            .one
            .conversation_group(std::slice::from_ref(&r.address))
            .await?;
        let me_user = self.my_username().await.unwrap_or_default();
        self.one
            .send_app(
                &gid,
                app::app_message::Body::ContactRequest(app::ContactRequest {
                    version: hashgram_app::version::PEOPLE_VERSION,
                    display_name: self.one.people_state.my_display_name.clone(),
                    username: me_user,
                    message: message.chars().take(p::MAX_REQUEST_MESSAGE).collect(),
                    at_ms: hashgram_app::ids::now_ms(),
                }),
            )
            .await?;
        self.one.people_state.save(&self.one.store)?;
        Ok(r)
    }

    /// Accepts or rejects a pending incoming request.
    pub async fn respond(&mut self, address: &str, accept: bool) -> Result<(), SdkError> {
        if accept {
            self.one.people_state.contacts.accept(address)?;
        } else {
            self.one.people_state.contacts.reject(address)?;
        }
        let gid = self.one.conversation_group(&[address.to_owned()]).await?;
        self.one
            .send_app(
                &gid,
                app::app_message::Body::ContactResponse(app::ContactResponse {
                    version: hashgram_app::version::PEOPLE_VERSION,
                    accepted: accept,
                    at_ms: hashgram_app::ids::now_ms(),
                }),
            )
            .await?;
        self.one.people_state.save(&self.one.store)
    }

    /// Removes a friend.
    pub fn remove(&mut self, address: &str) -> Result<(), SdkError> {
        self.one.people_state.contacts.remove(address)?;
        self.one.people_state.save(&self.one.store)
    }
    /// Blocks.
    pub fn block(&mut self, address: &str) -> Result<(), SdkError> {
        self.one.people_state.contacts.block(address)?;
        self.one.people_state.save(&self.one.store)
    }
    /// Unblocks.
    pub fn unblock(&mut self, address: &str) -> Result<(), SdkError> {
        self.one.people_state.contacts.unblock(address)?;
        self.one.people_state.save(&self.one.store)
    }
    /// Mute.
    pub fn mute(&mut self, address: &str, on: bool) -> Result<(), SdkError> {
        self.one.people_state.contacts.set_muted(address, on)?;
        self.one.people_state.save(&self.one.store)
    }
    /// Trust.
    pub fn trust(&mut self, address: &str, on: bool) -> Result<(), SdkError> {
        self.one.people_state.contacts.set_trusted(address, on)?;
        self.one.people_state.save(&self.one.store)
    }

    /// Follow / unfollow (public social event) and mirror locally.
    pub async fn follow(&mut self, address: &str, on: bool) -> Result<(), SdkError> {
        self.one.feed().set_follow(address, on).await?;
        self.one.people_state.contacts.set_following(address, on)?;
        self.one.people_state.save(&self.one.store)
    }

    /// Friends.
    pub fn friends(&self) -> Vec<app::ContactRecord> {
        self.one
            .people_state
            .contacts
            .with_flag(p::FRIEND)
            .into_iter()
            .cloned()
            .collect()
    }
    /// Incoming requests.
    pub fn incoming_requests(&self) -> Vec<app::ContactRecord> {
        self.one
            .people_state
            .contacts
            .with_flag(p::PENDING_IN)
            .into_iter()
            .cloned()
            .collect()
    }
    /// Outgoing requests.
    pub fn outgoing_requests(&self) -> Vec<app::ContactRecord> {
        self.one
            .people_state
            .contacts
            .with_flag(p::PENDING_OUT)
            .into_iter()
            .cloned()
            .collect()
    }
    /// Blocked.
    pub fn blocked(&self) -> Vec<app::ContactRecord> {
        self.one
            .people_state
            .contacts
            .with_flag(p::BLOCKED)
            .into_iter()
            .cloned()
            .collect()
    }
    /// Everyone in the book.
    pub fn all(&self) -> Vec<app::ContactRecord> {
        self.one
            .people_state
            .contacts
            .records
            .values()
            .cloned()
            .collect()
    }
    /// Local search over the contact book.
    pub fn search_local(&self, q: &str) -> Vec<app::ContactRecord> {
        let q = q.to_lowercase();
        self.one
            .people_state
            .contacts
            .records
            .values()
            .filter(|r| {
                r.address.contains(&q)
                    || r.username.to_lowercase().contains(&q)
                    || r.display_name.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }

    /// Sends our private profile card to a contact (optionally disclosing
    /// our wallet address to them).
    pub async fn send_card(
        &mut self,
        address: &str,
        bio: &str,
        disclose_wallet: bool,
    ) -> Result<(), SdkError> {
        if !self.one.people_state.contacts.has(address, p::FRIEND) {
            return Err(SdkError::Invalid("cards go to contacts only".into()));
        }
        let gid = self.one.conversation_group(&[address.to_owned()]).await?;
        let card = app::ProfileCard {
            version: hashgram_app::version::PEOPLE_VERSION,
            display_name: self.one.people_state.my_display_name.clone(),
            bio: bio.chars().take(4096).collect(),
            avatar: None,
            wallet_address: if disclose_wallet {
                self.one.account.address().to_owned()
            } else {
                String::new()
            },
            at_ms: hashgram_app::ids::now_ms(),
        };
        p::validate_card(&card)?;
        self.one
            .send_app(&gid, app::app_message::Body::ProfileCard(card))
            .await?;
        Ok(())
    }

    /// Handles People application messages.
    pub(crate) fn handle_incoming(
        &mut self,
        r: &Received,
        appmsg: &app::AppMessage,
    ) -> Result<bool, SdkError> {
        use app::app_message::Body as B;
        let changed = match &appmsg.body {
            Some(B::ContactRequest(req)) => {
                p::validate_request(req)?;
                self.one.people_state.contacts.request_received(
                    &r.sender,
                    &req.username,
                    &req.display_name,
                )?
            }
            Some(B::ContactResponse(resp)) => {
                self.one
                    .people_state
                    .contacts
                    .response_received(&r.sender, resp.accepted)?;
                true
            }
            Some(B::ProfileCard(card)) => {
                p::validate_card(card)?;
                if self.one.people_state.contacts.has(&r.sender, p::FRIEND) {
                    let username = self
                        .one
                        .people_state
                        .contacts
                        .records
                        .get(&r.sender)
                        .map(|x| x.username.clone())
                        .unwrap_or_default();
                    self.one.people_state.contacts.set_names(
                        &r.sender,
                        &username,
                        &card.display_name,
                    )?;
                    self.one
                        .store
                        .put(NS, format!("card/{}", r.sender).as_bytes(), card)?;
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if changed {
            self.one.people_state.save(&self.one.store)?;
        }
        Ok(changed)
    }

    /// The last card a contact sent us.
    pub fn card_of(&self, address: &str) -> Result<Option<app::ProfileCard>, SdkError> {
        self.one.store.get(NS, format!("card/{address}").as_bytes())
    }

    /// Merges a contacts snapshot from another of our devices.
    pub(crate) fn apply_snapshot(&mut self, s: &app::ContactsSnapshot) -> Result<(), SdkError> {
        self.one.people_state.contacts.merge_snapshot(s);
        self.one.people_state.save(&self.one.store)?;
        self.one.people_state.dirty = false;
        Ok(())
    }

    /// A snapshot for our other devices if anything changed.
    pub(crate) fn take_snapshot_if_dirty(&mut self) -> Option<app::ContactsSnapshot> {
        if self.one.people_state.dirty {
            self.one.people_state.dirty = false;
            Some(self.one.people_state.contacts.snapshot())
        } else {
            None
        }
    }
}
