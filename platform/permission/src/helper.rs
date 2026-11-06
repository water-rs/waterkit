//! The protocol between the app and the helper instances of itself that
//! read and request macOS privacy grants.
//!
//! macOS caches a negative grant answer in the process that asked, so
//! status is read in a fresh child of the app's executable, which prints a
//! [`GrantReport`] on stdout. A Screen Recording request from a plain child
//! never reaches TCC, so requests run in an instance `LaunchServices`
//! launches. Both run the app's own `main`, which hands them to
//! [`crate::run_helper_entry_if_requested`] before anything else.

use std::ffi::OsString;

use serde::{Deserialize, Serialize};

use crate::{Permission, PermissionStatus};

/// The first argument that marks a helper instance.
pub const ENTRY_ARG: &str = "--waterkit-permission-helper";

/// The helper command that prints a [`GrantReport`].
const STATUS_COMMAND: &str = "status";

/// The helper command that raises one grant's system prompt.
const REQUEST_COMMAND: &str = "request";

/// A macOS privacy grant that needs a helper instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    ScreenRecording,
    Accessibility,
    DirectScreenCapture,
}

impl Grant {
    /// The grant behind `permission`, if it is one of these.
    pub const fn from_permission(permission: Permission) -> Option<Self> {
        match permission {
            Permission::ScreenRecording => Some(Self::ScreenRecording),
            Permission::Accessibility => Some(Self::Accessibility),
            Permission::DirectScreenCapture => Some(Self::DirectScreenCapture),
            _ => None,
        }
    }

    /// How the request command names the grant.
    pub const fn arg(self) -> &'static str {
        match self {
            Self::ScreenRecording => "screen-recording",
            Self::Accessibility => "accessibility",
            Self::DirectScreenCapture => "direct-screen-capture",
        }
    }

    fn from_arg(arg: &str) -> Option<Self> {
        [
            Self::ScreenRecording,
            Self::Accessibility,
            Self::DirectScreenCapture,
        ]
        .into_iter()
        .find(|grant| grant.arg() == arg)
    }
}

/// What a helper instance was launched to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperCommand {
    /// Print a [`GrantReport`] on stdout.
    Status,
    /// Raise the system prompt for one grant.
    Request(Grant),
}

impl HelperCommand {
    /// The arguments, after [`ENTRY_ARG`], that launch this command.
    pub fn args(self) -> Vec<&'static str> {
        match self {
            Self::Status => vec![ENTRY_ARG, STATUS_COMMAND],
            Self::Request(grant) => vec![ENTRY_ARG, REQUEST_COMMAND, grant.arg()],
        }
    }

    /// The command `args` (the process's full argument list, program
    /// first) carries, or `None` when this is not a helper instance.
    ///
    /// # Panics
    ///
    /// Panics when the arguments carry [`ENTRY_ARG`] but no valid command:
    /// only this crate launches helper instances, so anything else is a bug.
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Option<Self> {
        let mut args = args.into_iter().skip(1);
        if args.next()? != ENTRY_ARG {
            return None;
        }
        let rest: Vec<String> = args
            .map(|arg| {
                arg.into_string()
                    .unwrap_or_else(|arg| panic!("helper argument {} is not UTF-8", arg.display()))
            })
            .collect();
        let command = match rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
            [STATUS_COMMAND] => Self::Status,
            [REQUEST_COMMAND, grant] => Self::Request(
                Grant::from_arg(grant).unwrap_or_else(|| panic!("unknown helper grant {grant:?}")),
            ),
            _ => panic!("malformed helper command {rest:?}"),
        };
        Some(command)
    }
}

/// The grants as a fresh process sees them; a status helper prints it as
/// JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantReport {
    /// `CGPreflightScreenCaptureAccess`.
    pub screen_recording: bool,
    /// `AXIsProcessTrusted`.
    pub accessibility: bool,
}

impl GrantReport {
    /// The report as a status helper prints it.
    pub fn encode(self) -> Vec<u8> {
        serde_json::to_vec(&self).expect("a grant report always serializes")
    }

    /// The report a status helper printed.
    ///
    /// # Errors
    ///
    /// Returns the parser's error when `bytes` is not a report.
    pub fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// The status of `grant` this report implies.
    ///
    /// Both TCC queries answer only "granted or not", so a missing grant is
    /// [`PermissionStatus::NotGranted`]. The direct-capture consent lives in
    /// `replayd`'s records, which the app cannot read: it is
    /// [`PermissionStatus::Unknown`] once Screen Recording is granted, and
    /// impossible before.
    pub const fn status(self, grant: Grant) -> PermissionStatus {
        let granted = match grant {
            Grant::ScreenRecording => self.screen_recording,
            Grant::Accessibility => self.accessibility,
            Grant::DirectScreenCapture if self.screen_recording => {
                return PermissionStatus::Unknown;
            }
            Grant::DirectScreenCapture => false,
        };
        if granted {
            PermissionStatus::Granted
        } else {
            PermissionStatus::NotGranted
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{ENTRY_ARG, Grant, GrantReport, HelperCommand};
    use crate::{Permission, PermissionStatus};

    const GRANTS: [Grant; 3] = [
        Grant::ScreenRecording,
        Grant::Accessibility,
        Grant::DirectScreenCapture,
    ];

    fn argv(args: &[&str]) -> Vec<OsString> {
        std::iter::once("app")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn ordinary_launches_are_not_helpers() {
        assert_eq!(HelperCommand::parse(argv(&[])), None);
        assert_eq!(HelperCommand::parse(argv(&["status"])), None);
        assert_eq!(HelperCommand::parse(argv(&["--verbose", ENTRY_ARG])), None);
        assert_eq!(HelperCommand::parse(Vec::new()), None);
    }

    #[test]
    fn every_command_parses_from_its_own_args() {
        let commands = std::iter::once(HelperCommand::Status)
            .chain(GRANTS.into_iter().map(HelperCommand::Request));
        for command in commands {
            assert_eq!(HelperCommand::parse(argv(&command.args())), Some(command));
        }
    }

    #[test]
    #[should_panic(expected = "unknown helper grant \"camera\"")]
    fn unknown_grant_fails_fast() {
        HelperCommand::parse(argv(&[ENTRY_ARG, "request", "camera"]));
    }

    #[test]
    #[should_panic(expected = "malformed helper command")]
    fn missing_command_fails_fast() {
        HelperCommand::parse(argv(&[ENTRY_ARG]));
    }

    #[test]
    #[should_panic(expected = "malformed helper command")]
    fn trailing_arguments_fail_fast() {
        HelperCommand::parse(argv(&[ENTRY_ARG, "status", "--request"]));
    }

    #[test]
    fn grants_map_from_their_permissions_only() {
        assert_eq!(
            Grant::from_permission(Permission::ScreenRecording),
            Some(Grant::ScreenRecording)
        );
        assert_eq!(
            Grant::from_permission(Permission::Accessibility),
            Some(Grant::Accessibility)
        );
        assert_eq!(
            Grant::from_permission(Permission::DirectScreenCapture),
            Some(Grant::DirectScreenCapture)
        );
        assert_eq!(Grant::from_permission(Permission::Camera), None);
    }

    #[test]
    fn report_round_trips() {
        for screen_recording in [false, true] {
            for accessibility in [false, true] {
                let report = GrantReport {
                    screen_recording,
                    accessibility,
                };
                assert_eq!(GrantReport::decode(&report.encode()).unwrap(), report);
            }
        }
    }

    #[test]
    fn a_launch_that_prints_something_else_is_no_report() {
        assert!(GrantReport::decode(b"").is_err());
        assert!(GrantReport::decode(b"{\"screen_recording\":true}").is_err());
    }

    #[test]
    fn tcc_grants_read_granted_or_not_granted() {
        let report = GrantReport {
            screen_recording: true,
            accessibility: false,
        };
        assert_eq!(
            report.status(Grant::ScreenRecording),
            PermissionStatus::Granted
        );
        assert_eq!(
            report.status(Grant::Accessibility),
            PermissionStatus::NotGranted
        );
        let report = GrantReport {
            screen_recording: false,
            accessibility: true,
        };
        assert_eq!(
            report.status(Grant::ScreenRecording),
            PermissionStatus::NotGranted
        );
        assert_eq!(
            report.status(Grant::Accessibility),
            PermissionStatus::Granted
        );
    }

    #[test]
    fn direct_capture_is_unknown_once_screen_recording_is_granted() {
        for accessibility in [false, true] {
            let without = GrantReport {
                screen_recording: false,
                accessibility,
            };
            let with = GrantReport {
                screen_recording: true,
                accessibility,
            };
            assert_eq!(
                without.status(Grant::DirectScreenCapture),
                PermissionStatus::NotGranted
            );
            assert_eq!(
                with.status(Grant::DirectScreenCapture),
                PermissionStatus::Unknown
            );
        }
    }
}
