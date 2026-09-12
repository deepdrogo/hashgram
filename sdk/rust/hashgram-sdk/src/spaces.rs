//! Spaces: shared environments (Family, Company, Project…) = an MLS group
//! plus a signed, hash-chained event log with roles.
//!
//! The rules (`hashgram_app::space::State::apply`) are enforced by every
//! member; this module signs our events with the device key, sends them in
//! the Space group, replays received ones, and reconciles MLS membership to
//! the role table (a `MemberAdd` triggers an MLS add, a removal an MLS
//! commit) so confidentiality follows authorisation.

use hashgram_app::pb as app;
use hashgram_app::space as sp;
use tracing::{debug, warn};

use crate::app::{group_kind, HashgramOne};
use crate::messaging::Received;
use crate::store::LocalStore;
use crate::SdkError;

const NS_EVENTS: &str = "spaces/events";
const NS_GROUP: &str = "spaces/group";

/// A space as listed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpaceSummary {
    /// Hex space id.
    pub id: String,
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Our role.
    pub my_role: i32,
    /// Member count.
    pub members: usize,
    /// Hex MLS group id.
    pub group_id: String,
    /// Created.
    pub created_at_ms: u64,
}

/// Spaces state: replayed `State` per space id (hex).
#[derive(Debug, Default)]
pub struct SpacesState {
    states: std::collections::HashMap<String, sp::State>,
    /// space id → group id (hex).
    groups: std::collections::HashMap<String, String>,
}

impl SpacesState {
    pub(crate) fn load(store: &LocalStore) -> Result<Self, SdkError> {
        let mut s = Self::default();
        for (k, g) in store.scan::<String>(NS_GROUP)? {
            s.groups.insert(String::from_utf8_lossy(&k).into_owned(), g);
        }
        Ok(s)
    }
}

/// The Spaces API.
pub struct Spaces<'a> {
    pub(crate) one: &'a mut HashgramOne,
}

impl<'a> Spaces<'a> {
    fn state_mut(&mut self, space_hex: &str) -> Result<&mut sp::State, SdkError> {
        if !self.one.spaces_state.states.contains_key(space_hex) {
            // Replay from the local event log.
            let sid = hex::decode(space_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
            let mut st = sp::State::new(&sid);
            let prefix = format!("{space_hex}/");
            let mut events: Vec<(String, app::SpaceEvent)> = self
                .one
                .store
                .scan::<(String, app::SpaceEvent)>(NS_EVENTS)?
                .into_iter()
                .filter(|(k, _)| k.starts_with(prefix.as_bytes()))
                .map(|(_, v)| v)
                .collect();
            events.sort_by_key(|(_, e)| (e.at_ms, e.event_id.clone()));
            for (sender, e) in events {
                let _ = st.apply(&self.one.network, &e, &sender);
            }
            self.one
                .spaces_state
                .states
                .insert(space_hex.to_owned(), st);
        }
        self.one
            .spaces_state
            .states
            .get_mut(space_hex)
            .ok_or_else(|| SdkError::NotFound("space".into()))
    }

    fn group_of(&self, space_hex: &str) -> Result<Vec<u8>, SdkError> {
        let g = self
            .one
            .spaces_state
            .groups
            .get(space_hex)
            .ok_or_else(|| SdkError::NotFound(format!("space {space_hex}")))?;
        hex::decode(g).map_err(|e| SdkError::Invalid(e.to_string()))
    }

    fn record(
        &mut self,
        space_hex: &str,
        sender: &str,
        e: &app::SpaceEvent,
    ) -> Result<(), SdkError> {
        let key = format!("{space_hex}/{:016x}/{}", e.at_ms, hex::encode(&e.event_id));
        self.one
            .store
            .put(NS_EVENTS, key.as_bytes(), &(sender.to_owned(), e.clone()))
    }

    /// Builds, signs, applies locally and sends an event. Local application
    /// first means our own rule violations are caught before anything
    /// leaves the device.
    async fn emit(
        &mut self,
        space_hex: &str,
        body: app::space_event::Body,
    ) -> Result<String, SdkError> {
        let me = self.one.account.address().to_owned();
        let device = self.one.account.device()?;
        let network = self.one.network.clone();
        let sid = hex::decode(space_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let (seq, head) = {
            let st = self.state_mut(space_hex)?;
            (st.next_sequence(&me), st.head_id())
        };
        let mut e = sp::build(&sid, &me, seq, &head, body)?;
        sp::sign(&network, &device, &mut e)?;
        {
            let st = self.state_mut(space_hex)?;
            st.apply(&network, &e, &me)
                .map_err(|r| SdkError::Invalid(format!("space rule: {r}")))?;
        }
        self.record(space_hex, &me, &e)?;
        let gid = self.group_of(space_hex)?;
        self.one
            .send_app(&gid, app::app_message::Body::SpaceEvent(e.clone()))
            .await?;
        Ok(hex::encode(&e.event_id))
    }

    /// Creates a Space. We become Owner; the MLS group starts with our
    /// devices only.
    pub async fn create(&mut self, name: &str, description: &str) -> Result<String, SdkError> {
        let me = self.one.account.address().to_owned();
        let gid = match self
            .one
            .messaging
            .create_conversation(
                &self.one.link,
                &self.one.chain,
                &self.one.network,
                name,
                &[],
            )
            .await
        {
            Ok(g) => g,
            Err(SdkError::NoRecipients) => {
                // Single-device account: MLS needs at least one other member
                // to create a Welcome, but a group with just us is fine.
                self.one
                    .messaging
                    .mls_mut()
                    .create_group(hashgram_mls::GroupMeta {
                        name: name.to_owned(),
                        direct: false,
                        joined_at: hashgram_app::ids::now_secs(),
                    })?
            }
            Err(e) => return Err(e),
        };
        let ghex = hex::encode(&gid);
        self.one.set_group_kind(&ghex, group_kind::SPACE)?;
        let sid = hashgram_app::ids::random_id()?;
        let shex = hex::encode(&sid);
        self.one.store.put(NS_GROUP, shex.as_bytes(), &ghex)?;
        self.one.spaces_state.groups.insert(shex.clone(), ghex);
        self.one
            .spaces_state
            .states
            .insert(shex.clone(), sp::State::new(&sid));
        self.emit(
            &shex,
            app::space_event::Body::Create(app::SpaceCreate {
                name: name.to_owned(),
                description: description.to_owned(),
                owner: me,
            }),
        )
        .await?;
        Ok(shex)
    }

    /// Invites an address with a role (Guest/Member/Admin). Emits the
    /// event first (so a rule violation is caught), then adds their
    /// devices to the MLS group and re-sends the full event log to them so
    /// they can replay the Space state.
    pub async fn invite(
        &mut self,
        space_hex: &str,
        address: &str,
        role: app::SpaceRole,
    ) -> Result<(), SdkError> {
        self.emit(
            space_hex,
            app::space_event::Body::MemberAdd(app::SpaceMemberAdd {
                address: address.to_owned(),
                role: role as i32,
            }),
        )
        .await?;
        let gid = self.group_of(space_hex)?;
        self.one
            .messaging
            .add_participant(
                &self.one.link,
                &self.one.chain,
                &self.one.network,
                &gid,
                address,
            )
            .await?;
        // History: a new member must see the log to replay roles.
        let prefix = format!("{space_hex}/");
        let mut events: Vec<(String, app::SpaceEvent)> = self
            .one
            .store
            .scan::<(String, app::SpaceEvent)>(NS_EVENTS)?
            .into_iter()
            .filter(|(k, _)| k.starts_with(prefix.as_bytes()))
            .map(|(_, v)| v)
            .collect();
        events.sort_by_key(|(_, e)| (e.at_ms, e.event_id.clone()));
        for (_, e) in events {
            if let Err(err) = self
                .one
                .send_app(&gid, app::app_message::Body::SpaceEvent(e))
                .await
            {
                debug!(error = %err, "space history replay send failed");
            }
        }
        Ok(())
    }

    /// Removes a member (or leaves when `address` is us).
    pub async fn remove(
        &mut self,
        space_hex: &str,
        address: &str,
        reason: &str,
    ) -> Result<(), SdkError> {
        self.emit(
            space_hex,
            app::space_event::Body::MemberRemove(app::SpaceMemberRemove {
                address: address.to_owned(),
                reason: reason.to_owned(),
            }),
        )
        .await?;
        let gid = self.group_of(space_hex)?;
        let _ = self
            .one
            .messaging
            .remove_participant(&self.one.link, &self.one.network, &gid, address)
            .await?;
        Ok(())
    }

    /// Changes a role.
    pub async fn set_role(
        &mut self,
        space_hex: &str,
        address: &str,
        role: app::SpaceRole,
    ) -> Result<String, SdkError> {
        self.emit(
            space_hex,
            app::space_event::Body::RoleChange(app::SpaceRoleChange {
                address: address.to_owned(),
                role: role as i32,
            }),
        )
        .await
    }

    /// Updates info.
    pub async fn set_info(
        &mut self,
        space_hex: &str,
        name: &str,
        description: &str,
    ) -> Result<String, SdkError> {
        self.emit(
            space_hex,
            app::space_event::Body::Info(app::SpaceInfoUpdate {
                name: name.to_owned(),
                description: description.to_owned(),
                avatar: None,
            }),
        )
        .await
    }

    /// Announcement (Admin+).
    pub async fn announce(
        &mut self,
        space_hex: &str,
        title: &str,
        text: &str,
        attachments: Vec<app::DriveCapability>,
    ) -> Result<String, SdkError> {
        self.emit(
            space_hex,
            app::space_event::Body::Announcement(app::SpaceAnnouncement {
                title: title.to_owned(),
                text: text.to_owned(),
                attachments,
            }),
        )
        .await
    }

    /// Post (Member+).
    pub async fn post(
        &mut self,
        space_hex: &str,
        text: &str,
        media: Vec<app::BlobRef>,
        drive_refs: Vec<app::DriveCapability>,
    ) -> Result<String, SdkError> {
        self.emit(
            space_hex,
            app::space_event::Body::Post(app::SpacePost {
                text: text.to_owned(),
                media,
                drive_refs,
            }),
        )
        .await
    }

    /// Comment (Member+).
    pub async fn comment(
        &mut self,
        space_hex: &str,
        post_hex: &str,
        text: &str,
    ) -> Result<String, SdkError> {
        self.emit(
            space_hex,
            app::space_event::Body::Comment(app::SpaceComment {
                post_id: hex::decode(post_hex).map_err(|e| SdkError::Invalid(e.to_string()))?,
                text: text.to_owned(),
            }),
        )
        .await
    }

    /// Shares one of our Drive entries into the Space drive at `path`.
    /// Live mode so the Space follows our edits.
    pub async fn share_drive(
        &mut self,
        space_hex: &str,
        entry_hex: &str,
        path: &str,
        live: bool,
    ) -> Result<String, SdkError> {
        let gid = self.group_of(space_hex)?;
        let id = hex::decode(entry_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let mode = if live {
            app::DriveShareMode::Live
        } else {
            app::DriveShareMode::Snapshot
        };
        let cap = self
            .one
            .drive()
            .grant_in_group(
                &id,
                &format!("space:{space_hex}"),
                &gid,
                mode,
                app::DrivePermission::Read,
            )
            .await?;
        let r = self
            .emit(
                space_hex,
                app::space_event::Body::DriveShare(app::SpaceDriveShare {
                    capability: Some(cap),
                    path: path.to_owned(),
                }),
            )
            .await;
        if r.is_ok() {
            let _ = self.one.drive().commit().await;
        }
        r
    }

    /// Unshares.
    pub async fn unshare_drive(
        &mut self,
        space_hex: &str,
        share_hex: &str,
    ) -> Result<String, SdkError> {
        self.emit(
            space_hex,
            app::space_event::Body::DriveUnshare(app::SpaceDriveUnshare {
                share_id: hex::decode(share_hex).map_err(|e| SdkError::Invalid(e.to_string()))?,
            }),
        )
        .await
    }

    /// Handles a Space event from the network.
    pub(crate) async fn handle_incoming(
        &mut self,
        r: &Received,
        appmsg: &app::AppMessage,
    ) -> Result<bool, SdkError> {
        let Some(app::app_message::Body::SpaceEvent(e)) = &appmsg.body else {
            return Ok(false);
        };
        let shex = hex::encode(&e.space_id);
        // Bind space → group on first sight; refuse a second group claiming
        // the same space (a member cannot move a space).
        match self.one.spaces_state.groups.get(&shex) {
            Some(g) if *g != r.group_id => {
                warn!("space event from a different group than the space is bound to; ignored");
                return Ok(false);
            }
            Some(_) => {}
            None => {
                self.one.set_group_kind(&r.group_id, group_kind::SPACE)?;
                self.one.store.put(NS_GROUP, shex.as_bytes(), &r.group_id)?;
                self.one
                    .spaces_state
                    .groups
                    .insert(shex.clone(), r.group_id.clone());
            }
        }
        let network = self.one.network.clone();
        let outcome = {
            let st = self.state_mut(&shex)?;
            st.apply(&network, e, &r.sender)
        };
        match outcome {
            Ok(()) | Err(sp::Rejected::Pending(_)) => {
                self.record(&shex, &r.sender, e)?;
                Ok(true)
            }
            Err(sp::Rejected::Duplicate) => Ok(false),
            Err(rej) => {
                debug!(%rej, "space event rejected");
                Ok(false)
            }
        }
    }

    /// Lists our spaces.
    pub fn list(&mut self) -> Result<Vec<SpaceSummary>, SdkError> {
        let me = self.one.account.address().to_owned();
        let ids: Vec<(String, String)> = self
            .one
            .spaces_state
            .groups
            .iter()
            .map(|(s, g)| (s.clone(), g.clone()))
            .collect();
        let mut out = Vec::new();
        for (sid, gid) in ids {
            let st = self.state_mut(&sid)?;
            if !st.exists() {
                continue;
            }
            out.push(SpaceSummary {
                id: sid.clone(),
                name: st.name.clone(),
                description: st.description.clone(),
                my_role: st.role_of(&me) as i32,
                members: st.members.len(),
                group_id: gid,
                created_at_ms: st.created_at_ms,
            });
        }
        out.sort_by_key(|s| s.name.to_lowercase());
        Ok(out)
    }

    /// Full replayed state.
    pub fn state(&mut self, space_hex: &str) -> Result<sp::State, SdkError> {
        Ok(self.state_mut(space_hex)?.clone())
    }

    /// Members sorted by rank.
    pub fn members(&mut self, space_hex: &str) -> Result<Vec<sp::Member>, SdkError> {
        Ok(self.state_mut(space_hex)?.members_sorted())
    }

    /// Content page (posts, comments, announcements), newest first.
    pub fn content(
        &mut self,
        space_hex: &str,
        before_ms: u64,
        limit: usize,
    ) -> Result<Vec<sp::Content>, SdkError> {
        Ok(self.state_mut(space_hex)?.content_page(before_ms, limit))
    }

    /// The Space drive listing.
    pub fn drive_entries(&mut self, space_hex: &str) -> Result<Vec<sp::SharedEntry>, SdkError> {
        let st = self.state_mut(space_hex)?;
        let mut v: Vec<sp::SharedEntry> = st.drive.values().cloned().collect();
        v.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then_with(|| a.capability.name.cmp(&b.capability.name))
        });
        Ok(v)
    }

    /// Sends mail to the whole Space (recipient set = members).
    pub async fn mail(
        &mut self,
        space_hex: &str,
        subject: &str,
        body: &str,
    ) -> Result<String, SdkError> {
        let me = self.one.account.address().to_owned();
        let members: Vec<String> = self
            .state_mut(space_hex)?
            .members
            .keys()
            .filter(|a| **a != me)
            .cloned()
            .collect();
        let to = self.one.mail().resolve_recipients(&members).await?;
        self.one
            .mail()
            .send(hashgram_app::mail::Draft {
                to,
                subject: subject.to_owned(),
                body_text: body.to_owned(),
                labels: vec![format!("space:{space_hex}")],
                ..Default::default()
            })
            .await
    }
}
