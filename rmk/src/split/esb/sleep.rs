//! ESB-local sleep management, independent of BLE.
//!
//! Watches [`SLEEP_INPUT`], stores the ESB sleep state in [`SLEEPING_STATE`],
//! and publishes [`SleepStateEvent`] for displays and split peripherals.
//! Only the peripheral stops its radio; the central keeps serving its links.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::select::{Either3, Either4, select3, select4};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};

use crate::SPLIT_CENTRAL_SLEEP_TIMEOUT_SECONDS;
use crate::event::{EventSubscriber, KeyboardEvent, PointingEvent, SleepStateEvent, SubscribableEvent, publish_event};

/// The latched sleep state.
/// - `true`: the keyboard is idle and sleeping
/// - `false`: the keyboard is awake
pub static SLEEPING_STATE: AtomicBool = AtomicBool::new(false);

/// Input to [`run_sleep_manager`], same encoding as [`SLEEPING_STATE`]:
/// - `true`: sleep now, without waiting out the idle timeout
/// - `false`: activity — wake up, or restart the idle timeout
static SLEEP_INPUT: Signal<crate::RawMutex, bool> = Signal::new();

/// Report keyboard activity: wake the keyboard up, or restart the idle timeout
/// when it's already awake.
pub fn report_activity() {
    SLEEP_INPUT.signal(false);
}

/// Ask the keyboard to sleep now instead of waiting out the idle timeout.
pub fn request_sleep() {
    SLEEP_INPUT.signal(true);
}

/// The keyboard's one sleep manager.
///
/// Run once on the ESB central, independently of individual connections.
/// Started by the ESB central macro. Disabled when the configured timeout
/// (`split_central_sleep_timeout_seconds`) is 0, the default.
/// USB sleep inhibition is temporarily disabled for testing over USB.
pub async fn run_sleep_manager() {
    if SPLIT_CENTRAL_SLEEP_TIMEOUT_SECONDS == 0 {
        info!("Sleep management disabled (timeout = 0)");
        core::future::pending::<()>().await;
        return;
    }

    info!(
        "Sleep manager started with {}s timeout",
        SPLIT_CENTRAL_SLEEP_TIMEOUT_SECONDS
    );
    manage_sleep_state(Duration::from_secs(SPLIT_CENTRAL_SLEEP_TIMEOUT_SECONDS.into())).await
}

/// Central idle tracking only: leave its radio running so peers can reconnect.
async fn manage_sleep_state(idle_timeout: Duration) -> ! {
    let mut keyboard = KeyboardEvent::subscriber();
    let mut pointing = PointingEvent::subscriber();
    let mut deadline = Instant::now() + idle_timeout;
    loop {
        if SLEEPING_STATE.load(Ordering::Acquire) {
            match select3(SLEEP_INPUT.wait(), keyboard.next_event(), pointing.next_event()).await {
                Either3::First(true) => {}
                _ => {
                    set_sleeping(false);
                    deadline = Instant::now() + idle_timeout;
                }
            }
        } else {
            match select4(
                SLEEP_INPUT.wait(),
                keyboard.next_event(),
                pointing.next_event(),
                Timer::at(deadline),
            )
            .await
            {
                Either4::First(true) | Either4::Fourth(_) => set_sleeping(true),
                _ => deadline = Instant::now() + idle_timeout,
            }
        }
    }
}

fn set_sleeping(sleeping: bool) {
    if SLEEPING_STATE.swap(sleeping, Ordering::AcqRel) != sleeping {
        info!("ESB central sleep state: {}", sleeping);
        publish_event(SleepStateEvent::new(sleeping));
    }
}

/// Follow the central's sleep state; local input wakes and resynchronizes this peripheral.
/// USB sleep inhibition is temporarily disabled for testing over USB.
pub async fn run_peripheral_sleep_manager() {
    info!("ESB peripheral sleep manager started");
    manage_peripheral_sleep(|sleeping| {
        if sleeping {
            yip::peripheral::stop();
            info!("ESB peripheral sleeping: radio stopped");
        } else {
            yip::peripheral::start();
            info!("ESB peripheral awake: radio started");
        }
    })
    .await
}

// The callback is monomorphized; tests substitute a radio call recorder.
async fn manage_peripheral_sleep(mut change_radio: impl FnMut(bool)) -> ! {
    let mut sleep = SleepStateEvent::subscriber();
    let mut keyboard = KeyboardEvent::subscriber();
    let mut pointing = PointingEvent::subscriber();
    loop {
        let sleeping = match select3(sleep.next_event(), keyboard.next_event(), pointing.next_event()).await {
            Either3::First(event) => {
                info!("ESB peripheral received sleep state: {}", event.0);
                event.0
            }
            _ => false,
        };
        if SLEEPING_STATE.swap(sleeping, Ordering::AcqRel) != sleeping {
            change_radio(sleeping);
            // Input can wake us before the central knows. Update local listeners;
            // receiving our own event later does not repeat the radio transition.
            if !sleeping {
                publish_event(SleepStateEvent::new(false));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_block_on;
    use core::sync::atomic::AtomicU32;
    use embassy_futures::select::select;
    use rmk_types::connection::UsbState;

    static STOPS: AtomicU32 = AtomicU32::new(0);
    static STARTS: AtomicU32 = AtomicU32::new(0);

    fn record_radio(sleeping: bool) {
        let counter = if sleeping { &STOPS } else { &STARTS };
        counter.fetch_add(1, Ordering::AcqRel);
    }

    fn run_peripheral(script: impl core::future::Future<Output = ()>) {
        test_block_on(async {
            SLEEPING_STATE.store(false, Ordering::Release);
            STOPS.store(0, Ordering::Release);
            STARTS.store(0, Ordering::Release);
            select(manage_peripheral_sleep(record_radio), script).await;
        });
    }

    #[test]
    fn central_idle_tracking_runs_with_usb_attached() {
        test_block_on(async {
            SLEEP_INPUT.reset();
            SLEEPING_STATE.store(false, Ordering::Release);
            crate::state::set_usb_state(UsbState::Configured);
            let script = async {
                Timer::after_millis(900).await;
                report_activity();
                Timer::after_millis(900).await;
                assert!(!SLEEPING_STATE.load(Ordering::Acquire));
                Timer::after_millis(200).await;
                assert!(SLEEPING_STATE.load(Ordering::Acquire));
                report_activity();
                Timer::after_millis(10).await;
                assert!(!SLEEPING_STATE.load(Ordering::Acquire));
            };
            select(manage_sleep_state(Duration::from_secs(1)), script).await;
        });
    }

    #[test]
    fn peripheral_sleep_stops_once_and_local_key_restarts() {
        run_peripheral(async {
            crate::state::set_usb_state(UsbState::Configured);
            publish_event(SleepStateEvent::new(true));
            Timer::after_millis(10).await;
            assert_eq!(STOPS.load(Ordering::Acquire), 1);
            publish_event(SleepStateEvent::new(true));
            Timer::after_millis(10).await;
            assert_eq!(STOPS.load(Ordering::Acquire), 1);
            publish_event(KeyboardEvent::key(0, 0, true));
            Timer::after_millis(10).await;
            assert!(!SLEEPING_STATE.load(Ordering::Acquire));
            assert_eq!(STARTS.load(Ordering::Acquire), 1);
        });
    }

    #[test]
    fn peripheral_pointing_and_awake_state_restart_without_duplicate_calls() {
        use crate::event::{Axis, AxisEvent, AxisValType};
        run_peripheral(async {
            publish_event(SleepStateEvent::new(true));
            Timer::after_millis(10).await;
            publish_event(PointingEvent {
                device_id: 0,
                axes: [AxisEvent {
                    typ: AxisValType::Rel,
                    axis: Axis::X,
                    value: 1,
                }; 3],
            });
            Timer::after_millis(10).await;
            assert_eq!(STARTS.load(Ordering::Acquire), 1);
            publish_event(SleepStateEvent::new(true));
            Timer::after_millis(10).await;
            publish_event(SleepStateEvent::new(false));
            Timer::after_millis(10).await;
            assert_eq!(STOPS.load(Ordering::Acquire), 2);
            assert_eq!(STARTS.load(Ordering::Acquire), 2);
        });
    }
}
