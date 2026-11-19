//! The tick driver (ADR-027 M2 amendment, item 2): each instance runs one tick step at start and
//! then one every [`TICK`] on a background thread, until exit begins. The interval is injected, so
//! a test drives ticks without sleeping 30 s. Part B starts it; nothing does in Part A.

use std::time::Duration;

use super::lifecycle::McpLifecycle;

/// The tick period.
pub const TICK: Duration = Duration::from_secs(30);

/// When the next tick comes. `false` ends the driver.
pub trait Interval {
    fn wait(&mut self) -> bool;
}

/// The production interval: sleep, then tick.
pub struct Every(pub Duration);

impl Interval for Every {
    fn wait(&mut self) -> bool {
        std::thread::sleep(self.0);
        true
    }
}

/// Run tick steps until exit begins or `interval` ends. A step that changes nothing (the row
/// unreadable, the lock held elsewhere, the switch off) is retried on the next tick: the driver
/// never gives up while the app runs.
pub fn run_ticks(lifecycle: &McpLifecycle, interval: &mut dyn Interval) {
    loop {
        lifecycle.tick_step();
        if lifecycle.is_exiting() || !interval.wait() {
            return;
        }
    }
}
