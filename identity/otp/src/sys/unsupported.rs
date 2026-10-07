use crate::{AppToken, OtpCapabilities, OtpError, Sender};

use super::Request;

pub const fn capabilities() -> OtpCapabilities {
    OtpCapabilities {
        available: false,
        addressed: None,
        consent: false,
        one_time_code_autofill: cfg!(any(target_os = "ios", target_os = "macos")),
    }
}

pub async fn start_addressed() -> Result<(AppToken, Request), OtpError> {
    Err(OtpError::Unavailable)
}

pub async fn start_consent(_sender: Option<Sender>) -> Result<Request, OtpError> {
    Err(OtpError::Unavailable)
}
