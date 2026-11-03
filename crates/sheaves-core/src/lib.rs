//! The portable core of Sheaves, shared by the Mac app (through UniFFI) and the
//! GNOME app (directly).

pub mod absence;
pub mod budget;
pub mod calendar_date;
pub mod client;
pub mod clock;
pub mod credentials;
pub mod hours;
pub mod models;
pub mod persistence;
pub mod queue;
pub mod snapshot;
pub mod tracked_entry;
pub mod tracker;

#[cfg(test)]
mod test_support;

uniffi::setup_scaffolding!();

/// The name and version of this library, such as `sheaves-core 0.1.0`, so a shell
/// can show which core it was built against.
#[uniffi::export]
pub fn core_version() -> String {
    format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_version_names_the_crate_and_its_version() {
        assert_eq!(
            core_version(),
            format!("sheaves-core {}", env!("CARGO_PKG_VERSION"))
        );
    }
}
