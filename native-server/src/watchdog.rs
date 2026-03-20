// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// systemd sd_notify integration (F19)
//
// When the `systemd` feature is enabled AND the process is started by systemd
// with Type=notify + WatchdogSec, this module:
//   1. Sends READY=1 after the listening socket is bound.
//   2. Spawns a background task that sends WATCHDOG=1 at WatchdogSec/2.
//   3. Sends STOPPING=1 on graceful shutdown.
//
// On non-Linux or without the feature: all functions are no-ops.
// ─────────────────────────────────────────────────────────────────────────────

/// Notify systemd that the service is ready (Type=notify).
/// Call this after the TCP listener is bound and the server is accepting requests.
pub fn notify_ready() {
    #[cfg(feature = "systemd")]
    {
        let _ = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]);
        tracing::info!("sd_notify: READY=1 sent");
    }
    #[cfg(not(feature = "systemd"))]
    {
        tracing::debug!("sd_notify: disabled (build without --features systemd)");
    }
}

/// Notify systemd that the service is stopping.
/// Call this at the beginning of the graceful shutdown sequence.
pub fn notify_stopping() {
    #[cfg(feature = "systemd")]
    {
        let _ = sd_notify::notify(true, &[sd_notify::NotifyState::Stopping]);
        tracing::info!("sd_notify: STOPPING=1 sent");
    }
}

/// Spawn a background task that sends WATCHDOG=1 at half the configured interval.
///
/// systemd sets `WATCHDOG_USEC` in the environment when `WatchdogSec=` is configured.
/// If the variable is absent (not running under systemd, or no watchdog configured),
/// this function returns without spawning anything.
pub fn spawn_watchdog_task() {
    #[cfg(feature = "systemd")]
    {
        // systemd sets WATCHDOG_USEC (microseconds) when WatchdogSec is configured
        let watchdog_usec = match std::env::var("WATCHDOG_USEC") {
            Ok(val) => match val.parse::<u64>() {
                Ok(us) if us > 0 => us,
                _ => {
                    tracing::debug!("sd_notify: WATCHDOG_USEC not parseable, watchdog disabled");
                    return;
                }
            },
            Err(_) => {
                tracing::debug!("sd_notify: WATCHDOG_USEC not set, watchdog disabled");
                return;
            }
        };

        // Send heartbeat at half the watchdog interval (systemd recommendation)
        let interval = std::time::Duration::from_micros(watchdog_usec / 2);
        tracing::info!(
            interval_ms = interval.as_millis(),
            watchdog_sec = watchdog_usec / 1_000_000,
            "sd_notify: watchdog heartbeat task started"
        );

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Watchdog]);
                tracing::trace!("sd_notify: WATCHDOG=1");
            }
        });
    }
    #[cfg(not(feature = "systemd"))]
    {
        // no-op: watchdog requires the systemd feature
    }
}
