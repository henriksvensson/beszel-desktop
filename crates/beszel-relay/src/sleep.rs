use std::time::{Duration, SystemTime};

use tokio::sync::watch;
use tokio::time::Instant;
use tracing::info;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const WAKE_JUMP_THRESHOLD: Duration = Duration::from_secs(4);

/// Spawn a lightweight background watcher that detects system suspend/resume.
///
/// On Linux, `CLOCK_MONOTONIC` (`Instant`) pauses during suspend while
/// `CLOCK_REALTIME` (`SystemTime`) advances. A forward divergence exceeding
/// `WAKE_JUMP_THRESHOLD` signals an immediate reconnect to drop stale sockets.
pub fn spawn_wake_detector(mut shutdown: watch::Receiver<bool>) -> watch::Receiver<u64> {
    let (tx, rx) = watch::channel(0u64);

    tokio::spawn(async move {
        let mut prev_wall = SystemTime::now();
        let mut prev_mono = Instant::now();
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;

        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    let curr_wall = SystemTime::now();
                    let curr_mono = Instant::now();
                    let wall_delta = curr_wall.duration_since(prev_wall).ok();
                    let mono_elapsed = curr_mono.duration_since(prev_mono);

                    if detect_sleep_jump(wall_delta, mono_elapsed, WAKE_JUMP_THRESHOLD) {
                        let gap_secs = wall_delta
                            .unwrap_or_default()
                            .saturating_sub(mono_elapsed)
                            .as_secs();
                        info!(gap_secs, "Detected system wake from sleep; triggering fast reconnect");
                        tx.send_modify(|count| *count = count.saturating_add(1));
                    }

                    prev_wall = curr_wall;
                    prev_mono = curr_mono;
                }
            }
        }
    });

    rx
}

/// Return `true` when wall clock jumped ahead of monotonic elapsed time by at least `threshold`.
pub fn detect_sleep_jump(
    wall_delta: Option<Duration>,
    mono_elapsed: Duration,
    threshold: Duration,
) -> bool {
    let Some(wall) = wall_delta else {
        return false;
    };
    wall.saturating_sub(mono_elapsed) >= threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_normal_ticks_and_backwards_clock_adjustments() {
        let threshold = Duration::from_secs(4);
        assert!(!detect_sleep_jump(
            Some(Duration::from_millis(1020)),
            Duration::from_millis(1000),
            threshold
        ));
        assert!(!detect_sleep_jump(None, Duration::from_secs(1), threshold));
    }

    #[test]
    fn detects_forward_wall_clock_jump_after_suspend() {
        let threshold = Duration::from_secs(4);
        assert!(detect_sleep_jump(
            Some(Duration::from_secs(120)),
            Duration::from_secs(1),
            threshold
        ));
    }
}
