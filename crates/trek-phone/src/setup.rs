//! Installing Trek Driver from the iPhone tool: what the computer lacks to build Trek's iPhone
//! app with the driver inside, and where an install stands. Plain types and decisions, the same
//! on every platform; the Mac's tool shows them (`route`), and a PC says through
//! [`Missing::Mac`] why the driver is installed from a Mac.

/// Installing Trek Driver from the iPhone tool: what this computer lacks first, then the phone
/// updater's steps as it builds Trek's iPhone app with the driver inside and installs it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Setup {
    #[default]
    Idle,
    /// Looking at what this Mac has to build the app with.
    Checking,
    /// The one thing this computer lacks to build the app.
    Missing(Missing),
    Building,
    Installing,
    /// Installed: the driver is read again.
    Installed,
    /// In the updater's words.
    Failed(String),
}

impl Setup {
    /// Under way: the install button waits.
    pub fn busy(&self) -> bool {
        matches!(self, Setup::Checking | Setup::Building | Setup::Installing)
    }
}

/// What a computer needs to build Trek's iPhone app, in the order it's looked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    /// Not a Mac: Trek Driver is built with Xcode, which only a Mac runs.
    Mac,
    Xcode,
    AppleId,
    Source,
}

impl Missing {
    pub fn line(self) -> &'static str {
        match self {
            Missing::Mac => {
                "Trek Driver is built and installed with Xcode, on a Mac. On a PC the iPhone tool shows the apps and files, and the screen once its developer disk image is mounted."
            }
            Missing::Xcode => {
                "Building Trek's iPhone app with its driver takes Xcode, which isn't on this Mac."
            }
            Missing::AppleId => {
                "Xcode needs your Apple ID to sign Trek's iPhone app: add it in Xcode › Settings › Accounts."
            }
            Missing::Source => {
                "Trek builds its iPhone app from its source code, which isn't on this Mac yet."
            }
        }
    }
}

/// The first thing a computer lacks to build Trek's iPhone app: being a Mac, then Xcode, an
/// Apple ID in Xcode, and Trek's source.
pub fn first_missing(mac: bool, xcode: bool, team: bool, source: bool) -> Option<Missing> {
    if !mac {
        Some(Missing::Mac)
    } else if !xcode {
        Some(Missing::Xcode)
    } else if !team {
        Some(Missing::AppleId)
    } else if !source {
        Some(Missing::Source)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_thing_a_computer_lacks_is_named() {
        assert_eq!(first_missing(false, true, true, true), Some(Missing::Mac));
        assert_eq!(
            first_missing(true, false, false, false),
            Some(Missing::Xcode)
        );
        assert_eq!(
            first_missing(true, true, false, false),
            Some(Missing::AppleId)
        );
        assert_eq!(
            first_missing(true, true, true, false),
            Some(Missing::Source)
        );
        assert_eq!(first_missing(true, true, true, true), None);
    }

    #[test]
    fn a_pc_is_told_where_the_driver_comes_from_and_what_works_here() {
        let line = Missing::Mac.line();
        assert!(
            line.contains("on a Mac") && line.contains("On a PC"),
            "{line}"
        );
        assert!(!line.contains("this Mac"), "a PC isn't a Mac: {line}");
        assert!(Setup::Checking.busy() && !Setup::Missing(Missing::Mac).busy());
    }
}
