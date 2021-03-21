use crate::linux::sandbox::ZygoteInfo;
use crossbeam_channel::TrySendError;
use parking_lot::Mutex;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(super) enum Event {
    CpuTle,
    RealTle,
    Heartbeat,
}

/// Monitors a sandbox, kills processes which used all their CPU time limit.
/// Limits are given in nanoseconds
#[tracing::instrument(skip(cpu_time_limit, real_time_limit, chan, driver, zygote))]
pub(super) async fn watchdog(
    jail_id: String,
    cpu_time_limit: u64,
    real_time_limit: u64,
    chan: crossbeam_channel::Sender<Event>,
    driver: Arc<crate::linux::cgroup::Driver>,
    zygote: Arc<Mutex<Option<ZygoteInfo>>>,
) {
    let start = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;

        // check that the sandbox is still alive
        match chan.try_send(Event::Heartbeat) {
            Ok(_) => {}
            Err(err) => match err {
                TrySendError::Disconnected(_) => {
                    tracing::info!("Sandbox was destroyed, exiting");
                    break;
                }
                _ => unreachable!("we use unbounded channel"),
            },
        }

        let start = start;
        let driver = driver.clone();
        let jail_id = jail_id.clone();
        let chan = chan.clone();
        let span = tracing::Span::current();
        let zygote = zygote.clone();
        let exited = tokio::task::spawn_blocking(move || {
            let _enter = span.enter();
            let elapsed = Instant::now().duration_since(start);
            let elapsed = elapsed.as_nanos() as u64;
            let current_usage = driver.get_cpu_usage(&jail_id).unwrap_or_else(|err| {
                tracing::error!("failed to get time usage: {:?}", err);
                tracing::error!("WARNING: assuming time limit exceeded");
                u64::max_value()
            });
            tracing::debug!(
                cpu_usage = current_usage,
                real_usage = elapsed,
                "collected time usage"
            );
            let was_cpu_tle = current_usage > cpu_time_limit;
            let was_real_tle = elapsed > real_time_limit;
            let ok = !was_cpu_tle && !was_real_tle;
            if ok {
                return false;
            }
            if was_cpu_tle {
                tracing::info!(
                    usage = current_usage,
                    limit = cpu_time_limit,
                    "CPU time limit exceeded"
                );
                let _ = chan.send(Event::CpuTle);
            } else if was_real_tle {
                tracing::info!(
                    usage = elapsed,
                    limit = real_time_limit,
                    "Real time limit exceeded"
                );
                let _ = chan.send(Event::RealTle);
            }
            let mut zyg = zygote.lock();
            {
                let zyg = zyg.as_ref();
                tracing::info!(pid = zyg.map_or(-1, |z| z.pid), "Killing sandbox");
            }
            zyg.take();
            true
        })
        .await
        .unwrap();
        if exited {
            tracing::info!("Sandbox was killed, exiting");
            break;
        }
    }
}
