//! Whether anybody is at Hostess: GNOME's idle clock and screen lock, read every
//! half minute and again the moment the screen unlocks or the machine wakes.
//!
//! What the readings mean is decided in the core (`PresenceMonitor::poll` in
//! `crates/sheaves-core/src/absence.rs`), which also asks the peers before
//! reporting an absence. This only reads the signals. There is no microphone
//! signal: nobody takes calls on this machine.

use std::sync::Arc;
use std::time::Duration;

use futures_lite::StreamExt;
use jiff::Timestamp;
use sheaves_core::absence::{Absence, PresenceReading};
use sheaves_core::ffi::PresenceFold;
use zbus::{Connection, proxy};

#[proxy(
    interface = "org.gnome.Mutter.IdleMonitor",
    default_service = "org.gnome.Mutter.IdleMonitor",
    default_path = "/org/gnome/Mutter/IdleMonitor/Core"
)]
trait IdleMonitor {
    /// Milliseconds since the last keyboard or pointer input.
    fn get_idletime(&self) -> zbus::Result<u64>;
}

#[proxy(
    interface = "org.gnome.ScreenSaver",
    default_service = "org.gnome.ScreenSaver",
    default_path = "/org/gnome/ScreenSaver"
)]
trait ScreenSaver {
    fn get_active(&self) -> zbus::Result<bool>;

    #[zbus(signal)]
    fn active_changed(&self, new_value: bool) -> zbus::Result<()>;
}

#[proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login {
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

/// One look at the machine, from what GNOME reports.
pub fn reading(idle_milliseconds: u64, screen_locked: bool) -> PresenceReading {
    PresenceReading {
        seconds_since_input: idle_milliseconds as f64 / 1000.0,
        microphone_in_use: false,
        screen_locked,
    }
}

/// Polls the machine until the process ends, sending each absence the core
/// reports. Runs on its own thread, with a runtime of its own.
pub fn watch(fold: Arc<PresenceFold>, absences: async_channel::Sender<Absence>) {
    std::thread::Builder::new()
        .name("sheaves-presence".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime can start");
            runtime.block_on(async move {
                if let Err(error) = run(fold, absences).await {
                    log::error!("not watching for absences: {error}");
                }
            });
        })
        .expect("a thread can start");
}

async fn run(
    fold: Arc<PresenceFold>,
    absences: async_channel::Sender<Absence>,
) -> zbus::Result<()> {
    let session = Connection::session().await?;
    let idle = IdleMonitorProxy::new(&session).await?;
    let screen = ScreenSaverProxy::new(&session).await?;

    // Unlocking and waking are not needed to measure an absence: the gap in the
    // evidence does that. They are what lets it be asked about the moment the user
    // is back, rather than up to half a minute later.
    let (wake, mut woken) = tokio::sync::mpsc::unbounded_channel::<&'static str>();
    let mut unlocked = screen.receive_active_changed().await?;
    let unlock = wake.clone();
    tokio::spawn(async move {
        while let Some(change) = unlocked.next().await {
            if change.args().is_ok_and(|args| !args.new_value) && unlock.send("unlock").is_err() {
                return;
            }
        }
    });
    match Connection::system().await {
        Ok(system) => match LoginProxy::new(&system).await {
            Ok(login) => {
                let mut sleeps = login.receive_prepare_for_sleep().await?;
                tokio::spawn(async move {
                    // Hold the connection for as long as the signal is wanted.
                    let _system = system;
                    while let Some(sleep) = sleeps.next().await {
                        if sleep.args().is_ok_and(|args| !args.start) && wake.send("wake").is_err()
                        {
                            return;
                        }
                    }
                });
            }
            Err(error) => log::warn!("will not notice waking from sleep: {error}"),
        },
        Err(error) => log::warn!("will not notice waking from sleep: {error}"),
    }

    let mut every = tokio::time::interval(Duration::from_secs_f64(
        sheaves_core::absence::PresenceMonitor::POLL_INTERVAL,
    ));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = every.tick() => {}
            Some(why) = woken.recv() => log::info!("looking now: {why}"),
        }
        let (idle_milliseconds, locked) =
            match (idle.get_idletime().await, screen.get_active().await) {
                (Ok(idle), Ok(locked)) => (idle, locked),
                (Err(error), _) | (_, Err(error)) => {
                    log::warn!("could not read the machine: {error}");
                    continue;
                }
            };
        for absence in fold
            .poll(reading(idle_milliseconds, locked), Timestamp::now())
            .await
        {
            log::info!(
                "absence of {:.1} minutes ending {}",
                absence.duration() / 60.0,
                absence.ended
            );
            if absences.send(absence).await.is_err() {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_milliseconds_as_seconds_and_never_hears_a_microphone() {
        let reading = reading(90_500, true);
        assert_eq!(reading.seconds_since_input, 90.5);
        assert!(reading.screen_locked);
        assert!(!reading.microphone_in_use);
    }
}
