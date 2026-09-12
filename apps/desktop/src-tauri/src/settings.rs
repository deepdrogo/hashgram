//! User settings. No secrets live here; the file is plain JSON.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Which network the app is pinned to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkKind {
    /// Hashgram Mainnet: genesis hash compiled in.
    Mainnet,
    /// A developer network. Visually unmistakable (persistent banner).
    Devnet,
}

/// Network profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkSettings {
    /// Mainnet or devnet.
    pub kind: NetworkKind,
    /// Devnet genesis hash (ignored on mainnet, which is compiled in).
    #[serde(default)]
    pub devnet_genesis_hash: String,
    /// Devnet bootstrap multiaddrs (mainnet uses the compiled-in list plus
    /// the peerstore).
    #[serde(default)]
    pub devnet_bootstrap: Vec<String>,
    /// HTTPS REST endpoints the user pasted; precedence 3, never the only
    /// way in.
    #[serde(default)]
    pub https_endpoints: Vec<String>,
    /// A chain node's REST gateway on this PC; precedence 1.
    #[serde(default = "default_local_node_api")]
    pub local_node_api: String,
}

fn default_local_node_api() -> String {
    "http://127.0.0.1:1317".to_owned()
}

/// Security settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecuritySettings {
    /// Minutes of inactivity before the vault locks (0 = never).
    #[serde(default = "default_auto_lock")]
    pub auto_lock_minutes: u32,
    /// Whether Windows Hello unlock is enrolled.
    #[serde(default)]
    pub hello_enabled: bool,
    /// Seconds before a sensitive clipboard copy is cleared.
    #[serde(default = "default_clipboard_clear")]
    pub clipboard_clear_secs: u32,
}

fn default_auto_lock() -> u32 {
    15
}
fn default_clipboard_clear() -> u32 {
    30
}

/// Appearance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AppearanceSettings {
    /// Reduce motion regardless of the OS setting.
    #[serde(default)]
    pub reduced_motion: bool,
    /// Compact density.
    #[serde(default)]
    pub compact: bool,
}

/// Notifications.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationSettings {
    /// Toast on new message.
    #[serde(default = "default_true")]
    pub messages: bool,
    /// Toast on incoming call.
    #[serde(default = "default_true")]
    pub calls: bool,
}

fn default_true() -> bool {
    true
}

/// Media.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaSettings {
    /// Autoplay reels and videos.
    #[serde(default = "default_true")]
    pub autoplay: bool,
    /// Media cache ceiling in MiB.
    #[serde(default = "default_cache_mb")]
    pub cache_mb: u32,
}

fn default_cache_mb() -> u32 {
    2048
}

/// Updates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateSettings {
    /// Check on start (only outbound HTTPS besides user endpoints).
    #[serde(default = "default_true")]
    pub auto_check: bool,
    /// Channel name.
    #[serde(default = "default_channel")]
    pub channel: String,
}

fn default_channel() -> String {
    "stable".to_owned()
}

/// Advanced.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdvancedSettings {
    /// tracing filter.
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_log_level() -> String {
    "info".to_owned()
}

/// All settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    /// Network.
    pub network: NetworkSettings,
    /// Security.
    pub security: SecuritySettings,
    /// Appearance.
    #[serde(default)]
    pub appearance: AppearanceSettings,
    /// Notifications.
    pub notifications: NotificationSettings,
    /// Media.
    pub media: MediaSettings,
    /// Updates.
    pub updates: UpdateSettings,
    /// Advanced.
    pub advanced: AdvancedSettings,
    /// Start with Windows.
    #[serde(default)]
    pub start_with_windows: bool,
    /// Onboarding completed (a vault exists and was confirmed).
    #[serde(default)]
    pub onboarding_done: bool,
    /// Label this PC registered its device key under.
    #[serde(default)]
    pub device_label: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            network: NetworkSettings {
                kind: NetworkKind::Mainnet,
                devnet_genesis_hash: String::new(),
                devnet_bootstrap: Vec::new(),
                https_endpoints: Vec::new(),
                local_node_api: default_local_node_api(),
            },
            security: SecuritySettings {
                auto_lock_minutes: default_auto_lock(),
                hello_enabled: false,
                clipboard_clear_secs: default_clipboard_clear(),
            },
            appearance: AppearanceSettings::default(),
            notifications: NotificationSettings {
                messages: true,
                calls: true,
            },
            media: MediaSettings {
                autoplay: true,
                cache_mb: default_cache_mb(),
            },
            updates: UpdateSettings {
                auto_check: true,
                channel: default_channel(),
            },
            advanced: AdvancedSettings {
                log_level: default_log_level(),
            },
            start_with_windows: false,
            onboarding_done: false,
            device_label: String::new(),
        }
    }
}

impl Settings {
    /// Loads settings, falling back to defaults when the file is absent or
    /// unreadable (a corrupt settings file must never lock a user out).
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "settings unreadable; using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Writes settings atomically.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_mainnet_with_no_endpoints() {
        let s = Settings::default();
        assert_eq!(s.network.kind, NetworkKind::Mainnet);
        assert!(s.network.https_endpoints.is_empty());
        assert!(!s.onboarding_done);
    }

    #[test]
    fn round_trips_and_tolerates_garbage() {
        let d = std::env::temp_dir().join(format!("hg-settings-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let p = d.join("settings.json");
        let mut s = Settings::default();
        s.network
            .https_endpoints
            .push("https://rest.example.org".into());
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
        std::fs::write(&p, b"{not json").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
    }
}
