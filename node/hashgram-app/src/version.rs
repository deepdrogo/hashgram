//! Protocol generations and the single reader-side compatibility rule.

use crate::AppError;

/// `AppMessage.version` this build writes.
pub const APP_ENVELOPE_VERSION: u32 = 1;
/// Highest `AppMessage.version` this build can read.
pub const APP_ENVELOPE_MAX_READ: u32 = 1;

/// `MailMessage.version` this build writes / can read.
pub const MAIL_VERSION: u32 = 1;
/// `DriveObjectRef.version` and `DriveManifest.version`.
pub const DRIVE_VERSION: u32 = 1;
/// `DriveCapability.version`.
pub const CAPABILITY_VERSION: u32 = 1;
/// `SpaceEvent.version`.
pub const SPACE_VERSION: u32 = 1;
/// `CircleEvent.version`.
pub const CIRCLE_VERSION: u32 = 1;
/// `ContactRequest.version` etc.
pub const PEOPLE_VERSION: u32 = 1;
/// `DeviceSync.version`.
pub const DEVICE_SYNC_VERSION: u32 = 1;

/// Checks a message's declared version against what this build reads.
///
/// `min_reader` is the sender's "do not guess below this" bound; `0` means
/// none. `version == 0` is the proto3 default and means the sender forgot,
/// which is treated as version 1 for the envelope only (so an old client
/// that never set the field still decodes) but is an error for every body,
/// where the field has always been mandatory.
pub fn check(what: &str, version: u32, min_reader: u32, max_read: u32) -> Result<(), AppError> {
    if version == 0 || version > max_read {
        return Err(AppError::Unsupported(format!(
            "{what} version {version}; this build reads up to {max_read}"
        )));
    }
    if min_reader > max_read {
        return Err(AppError::Unsupported(format!(
            "{what} requires reader version {min_reader}; this build is {max_read}"
        )));
    }
    Ok(())
}

/// Same as [`check`] for a body type with a single supported version.
pub fn check_body(what: &str, version: u32, supported: u32) -> Result<(), AppError> {
    if version != supported {
        return Err(AppError::Unsupported(format!(
            "{what} version {version}; this build supports {supported}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_versions() {
        assert!(check("env", 1, 0, 1).is_ok());
        assert!(check("env", 1, 1, 1).is_ok());
        assert!(check("env", 2, 0, 1).is_err());
        assert!(check("env", 1, 2, 1).is_err());
        assert!(check("env", 0, 0, 1).is_err());
    }

    #[test]
    fn body_versions() {
        assert!(check_body("mail", 1, 1).is_ok());
        assert!(check_body("mail", 2, 1).is_err());
        assert!(check_body("mail", 0, 1).is_err());
    }
}
