//! The sync engine: one explicit state machine that brings every
//! application up to date, resumably and idempotently.
//!
//! ```text
//!  Offline ──connect──▶ Connecting ──first verified peer──▶ Discovering
//!     ▲                     │ timeout                          │ peers/roles known
//!     │                     ▼                                  ▼
//!     └───── Backoff(n) ◀── (error) ◀───────────────────── Syncing{stage}
//!                                                              │ all stages done
//!                                                              ▼
//!                                                            Idle ──tick──▶ Syncing
//! ```
//!
//! Stages inside `Syncing`, in order: `Mailbox` (fetch envelopes from every
//! store peer holding our mailbox, decrypt, dispatch to Mail / Drive /
//! People / Circles / Spaces / Devices), `Outbox` (flags hint, contacts
//! snapshot, Drive keyring to our other devices; Drive manifest commit if
//! dirty), `Feed` (refresh followed authors), `Wallet` (balance), `Devices`
//! (reconcile every N rounds). Every stage is resumable: the mailbox keeps
//! a cursor per store peer, uploads resume through `missing_chunks`, the
//! Drive keeps a revision, and application messages are de-duplicated by
//! id, so a round interrupted anywhere can simply run again.
//!
//! No UI assumptions: the engine reports [`SyncEvent`]s through an optional
//! channel and returns a [`SyncReport`] per round.

use std::time::{Duration, Instant};

use hashgram_app::envelope;
use tracing::{debug, info, warn};

use crate::app::HashgramOne;
use crate::messaging::Received;
use crate::SdkError;

/// Engine state.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum SyncPhase {
    /// No link.
    Offline,
    /// Link started, no verified peer yet.
    Connecting,
    /// Peers verified; learning roles/providers.
    Discovering,
    /// Running a stage.
    Syncing(Stage),
    /// Up to date.
    Idle,
    /// Waiting after an error; `attempt` drives exponential backoff.
    Backoff {
        /// Attempt number.
        attempt: u32,
        /// Seconds until the next try.
        wait_secs: u64,
    },
}

/// Stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Stage {
    /// Mailbox fetch + dispatch.
    Mailbox,
    /// Outgoing device-sync + Drive commit.
    Outbox,
    /// Feed refresh.
    Feed,
    /// Wallet balance.
    Wallet,
    /// Device reconciliation.
    Devices,
}

/// Events the engine emits.
#[derive(Debug, Clone, serde::Serialize)]
pub enum SyncEvent {
    /// Phase change.
    Phase(SyncPhase),
    /// New mail filed (id hex, folder).
    NewMail {
        /// Id.
        id: String,
        /// Folder.
        folder: String,
    },
    /// A Drive share arrived or changed.
    DriveShareChanged,
    /// Contacts changed.
    ContactsChanged,
    /// Circle activity (circle hex).
    CircleActivity(String),
    /// Space activity (space hex).
    SpaceActivity(String),
    /// Balance (uhash).
    Balance(String),
    /// An application message this build cannot read (kind kept opaque).
    UnsupportedMessage,
    /// A non-fatal error.
    Warning(String),
}

/// Summary of one round.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SyncReport {
    /// Envelopes processed.
    pub envelopes: usize,
    /// Mail filed.
    pub mail: usize,
    /// Drive share changes.
    pub drive: usize,
    /// People changes.
    pub people: usize,
    /// Circle events.
    pub circles: usize,
    /// Space events.
    pub spaces: usize,
    /// Device-sync messages.
    pub device_sync: usize,
    /// Unsupported messages kept.
    pub unsupported: usize,
    /// Legacy chat lines (handed back for the caller's chat UI, if any).
    pub chat: Vec<Received>,
    /// Feed events fetched.
    pub feed: usize,
    /// Drive committed revision (if a commit happened).
    pub drive_committed: Option<u64>,
    /// Duration.
    pub elapsed_ms: u64,
    /// Balance (uhash) if fetched.
    pub balance_uhash: Option<u128>,
}

/// Persistent engine state.
#[derive(Debug)]
pub struct SyncState {
    /// Phase.
    pub phase: SyncPhase,
    /// Rounds completed.
    pub rounds: u64,
    /// Consecutive failures.
    pub failures: u32,
    /// Last successful round.
    pub last_ok: Option<Instant>,
    /// Event sink.
    pub events: Option<tokio::sync::mpsc::UnboundedSender<SyncEvent>>,
    /// Reconcile devices every N rounds.
    pub reconcile_every: u64,
}

impl Default for SyncState {
    fn default() -> Self {
        Self {
            phase: SyncPhase::Offline,
            rounds: 0,
            failures: 0,
            last_ok: None,
            events: None,
            reconcile_every: 20,
        }
    }
}

/// Backoff schedule: 2, 4, 8 … capped at 300 s.
#[must_use]
pub fn backoff_secs(attempt: u32) -> u64 {
    (2u64.saturating_pow(attempt.min(9))).clamp(2, 300)
}

/// The Sync API.
pub struct Sync<'a> {
    pub(crate) one: &'a mut HashgramOne,
}

impl<'a> Sync<'a> {
    fn set_phase(&mut self, p: SyncPhase) {
        if self.one.sync_state.phase != p {
            debug!(?p, "sync phase");
            self.one.sync_state.phase = p.clone();
            self.emit(SyncEvent::Phase(p));
        }
    }

    fn emit(&self, e: SyncEvent) {
        if let Some(tx) = &self.one.sync_state.events {
            let _ = tx.send(e);
        }
    }

    /// Subscribes to events.
    pub fn subscribe(&mut self) -> tokio::sync::mpsc::UnboundedReceiver<SyncEvent> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.one.sync_state.events = Some(tx);
        rx
    }

    /// Current phase.
    pub fn phase(&self) -> SyncPhase {
        self.one.sync_state.phase.clone()
    }

    /// Runs one full round. Never panics on bad input; returns the report
    /// or the error that stopped the round (the phase is then `Backoff`).
    pub async fn round(&mut self) -> Result<SyncReport, SdkError> {
        let started = Instant::now();
        if self.one.sync_state.rounds == 0 {
            // Rounds are persisted so periodic work (device reconciliation)
            // is spread over real rounds rather than repeated at every
            // process start.
            self.one.sync_state.rounds = self
                .one
                .store
                .get("sync/meta", b"rounds")
                .ok()
                .flatten()
                .unwrap_or(0);
        }
        match self.round_inner().await {
            Ok(mut r) => {
                r.elapsed_ms = started.elapsed().as_millis() as u64;
                self.one.sync_state.rounds += 1;
                let _ = self
                    .one
                    .store
                    .put("sync/meta", b"rounds", &self.one.sync_state.rounds);
                self.one.sync_state.failures = 0;
                self.one.sync_state.last_ok = Some(Instant::now());
                self.set_phase(SyncPhase::Idle);
                Ok(r)
            }
            Err(e) => {
                self.one.sync_state.failures += 1;
                let attempt = self.one.sync_state.failures;
                self.set_phase(SyncPhase::Backoff {
                    attempt,
                    wait_secs: backoff_secs(attempt),
                });
                Err(e)
            }
        }
    }

    /// How long to wait before the next round given the current phase.
    pub fn next_delay(&self) -> Duration {
        match &self.one.sync_state.phase {
            SyncPhase::Backoff { wait_secs, .. } => Duration::from_secs(*wait_secs),
            _ => Duration::from_secs(4),
        }
    }

    async fn round_inner(&mut self) -> Result<SyncReport, SdkError> {
        let mut report = SyncReport::default();
        // Connecting / Discovering.
        let peers = self.one.link.peers().await;
        if peers.is_empty() {
            self.set_phase(SyncPhase::Connecting);
            return Err(SdkError::Link(crate::link::LinkError::NoPeer("any")));
        }
        self.set_phase(SyncPhase::Discovering);
        let stores = self.one.link.peers_with_role("store").await;
        if stores.is_empty() {
            self.emit(SyncEvent::Warning(
                "no store node reachable; mailbox not synced".into(),
            ));
        }

        // Mailbox.
        self.set_phase(SyncPhase::Syncing(Stage::Mailbox));
        let received = self
            .one
            .messaging
            .sync(&self.one.link, &self.one.network, Some(&self.one.chain))
            .await?;
        report.envelopes = received.len();
        for r in received {
            self.dispatch(r, &mut report).await;
        }

        // Outbox.
        self.set_phase(SyncPhase::Syncing(Stage::Outbox));
        if let Some(hint) = self.one.mail().take_pending_hint() {
            if let Some(gid) = self.one.self_group().await? {
                let body = hashgram_app::pb::DeviceSync {
                    version: hashgram_app::version::DEVICE_SYNC_VERSION,
                    body: Some(hashgram_app::pb::device_sync::Body::MailState(hint)),
                };
                if let Err(e) = self
                    .one
                    .send_app(&gid, hashgram_app::pb::app_message::Body::DeviceSync(body))
                    .await
                {
                    self.emit(SyncEvent::Warning(format!(
                        "mail flags not synced to other devices: {e}"
                    )));
                }
            }
        }
        if let Some(snapshot) = self.one.people().take_snapshot_if_dirty() {
            if let Some(gid) = self.one.self_group().await? {
                let body = hashgram_app::pb::DeviceSync {
                    version: hashgram_app::version::DEVICE_SYNC_VERSION,
                    body: Some(hashgram_app::pb::device_sync::Body::Contacts(snapshot)),
                };
                let _ = self
                    .one
                    .send_app(&gid, hashgram_app::pb::app_message::Body::DeviceSync(body))
                    .await;
            }
        }
        if self.one.drive_state.dirty {
            match self.one.drive().commit().await {
                Ok(rev) => report.drive_committed = Some(rev),
                Err(e) => self.emit(SyncEvent::Warning(format!("drive commit deferred: {e}"))),
            }
        }
        let _ = self.one.mail().purge();

        // Feed.
        self.set_phase(SyncPhase::Syncing(Stage::Feed));
        match self.one.feed().refresh().await {
            Ok(n) => report.feed = n,
            Err(e) => self.emit(SyncEvent::Warning(format!("feed refresh: {e}"))),
        }

        // Wallet.
        self.set_phase(SyncPhase::Syncing(Stage::Wallet));
        let me = self.one.account.address().to_owned();
        match self.one.chain.balance(&me).await {
            Ok(b) => {
                report.balance_uhash = Some(b);
                self.emit(SyncEvent::Balance(b.to_string()));
            }
            Err(crate::chain::ClientError::NoAccount(_)) => {
                report.balance_uhash = Some(0);
                self.emit(SyncEvent::Balance("0".into()));
            }
            Err(e) => self.emit(SyncEvent::Warning(format!("balance: {e}"))),
        }

        // Devices.
        let rounds = self.one.sync_state.rounds;
        if rounds > 0 && rounds.is_multiple_of(self.one.sync_state.reconcile_every.max(1)) {
            self.set_phase(SyncPhase::Syncing(Stage::Devices));
            match self.one.devices().reconcile().await {
                Ok(rep) => {
                    if !rep.removed.is_empty() || !rep.added.is_empty() {
                        info!(
                            removed = rep.removed.len(),
                            added = rep.added.len(),
                            "device reconciliation"
                        );
                    }
                }
                Err(e) => self.emit(SyncEvent::Warning(format!("device reconcile: {e}"))),
            }
        }
        self.one.save()?;
        Ok(report)
    }

    /// Routes one decrypted message to its application.
    async fn dispatch(&mut self, r: Received, report: &mut SyncReport) {
        let Some(appmsg) = HashgramOne::app_of(&r).cloned() else {
            report.chat.push(r);
            return;
        };
        // Version gate (the transport already decoded; re-check here so an
        // unknown body is reported rather than silently dropped).
        if appmsg.body.is_none() || appmsg.version > hashgram_app::version::APP_ENVELOPE_MAX_READ {
            report.unsupported += 1;
            self.emit(SyncEvent::UnsupportedMessage);
            let _ = self.one.store.put(
                "unsupported",
                &appmsg.id,
                &(
                    r.group_id.clone(),
                    r.sender.clone(),
                    r.raw.encode_to_vec_hex(),
                ),
            );
            return;
        }
        use hashgram_app::pb::app_message::Body as B;
        let kind = envelope::kind_name(&appmsg);
        let result: Result<(), SdkError> = match &appmsg.body {
            Some(B::Mail(_)) | Some(B::MailReceipt(_)) => {
                match self.one.mail().handle_incoming(&r, &appmsg).await {
                    Ok(Some(rec)) => {
                        report.mail += 1;
                        self.emit(SyncEvent::NewMail {
                            id: hex::encode(&rec.message.message_id),
                            folder: rec.folder,
                        });
                        Ok(())
                    }
                    Ok(None) => Ok(()),
                    Err(e) => Err(e),
                }
            }
            Some(B::DriveShare(_))
            | Some(B::DriveShareUpdate(_))
            | Some(B::DriveShareRevoke(_)) => match self.one.drive().handle_incoming(&r, &appmsg) {
                Ok(true) => {
                    report.drive += 1;
                    self.emit(SyncEvent::DriveShareChanged);
                    Ok(())
                }
                Ok(false) => Ok(()),
                Err(e) => Err(e),
            },
            Some(B::ContactRequest(_)) | Some(B::ContactResponse(_)) | Some(B::ProfileCard(_)) => {
                match self.one.people().handle_incoming(&r, &appmsg) {
                    Ok(true) => {
                        report.people += 1;
                        self.emit(SyncEvent::ContactsChanged);
                        Ok(())
                    }
                    Ok(false) => Ok(()),
                    Err(e) => Err(e),
                }
            }
            Some(B::CircleEvent(_)) => match self.one.circles().handle_incoming(&r, &appmsg) {
                Ok(true) => {
                    report.circles += 1;
                    self.emit(SyncEvent::CircleActivity(r.group_id.clone()));
                    Ok(())
                }
                Ok(false) => Ok(()),
                Err(e) => Err(e),
            },
            Some(B::SpaceEvent(e)) => match self.one.spaces().handle_incoming(&r, &appmsg).await {
                Ok(true) => {
                    report.spaces += 1;
                    self.emit(SyncEvent::SpaceActivity(hex::encode(&e.space_id)));
                    Ok(())
                }
                Ok(false) => Ok(()),
                Err(err) => Err(err),
            },
            Some(B::DeviceSync(s)) => {
                match self.one.devices().handle_incoming(&r.sender, s).await {
                    Ok(_) => {
                        report.device_sync += 1;
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
            None => Ok(()),
        };
        if let Err(e) = result {
            warn!(kind, error = %e, "application message rejected");
            self.emit(SyncEvent::Warning(format!("{kind}: {e}")));
        }
    }
}

trait EncodeHex {
    fn encode_to_vec_hex(&self) -> String;
}

impl EncodeHex for hashgram_proto::chat::ChatMessage {
    fn encode_to_vec_hex(&self) -> String {
        use prost::Message;
        hex::encode(self.encode_to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_bounded() {
        assert_eq!(backoff_secs(0), 2);
        assert_eq!(backoff_secs(1), 2);
        assert_eq!(backoff_secs(2), 4);
        assert_eq!(backoff_secs(3), 8);
        assert_eq!(backoff_secs(20), 300);
    }
}
