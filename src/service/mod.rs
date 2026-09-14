//! Installing the daemon as a user service (launchd on macOS, systemd on
//! Linux).

#[cfg(target_os = "macos")]
pub mod launchd;

#[cfg(target_os = "linux")]
pub mod systemd;

/// Whether the daemon user service is installed on this host.
pub fn is_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        launchd::is_installed()
    }
    #[cfg(target_os = "linux")]
    {
        systemd::is_installed()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}
