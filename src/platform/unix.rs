//! macOS and Linux: a plain loop. Wakes every few seconds, or right away after a change.

use crate::service::{self, Service};
use crate::{engine, ipc, procs, util};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

pub fn run_service() {
    if service::already_running() {
        return;
    }
    let svc = Arc::new(Mutex::new(Service::new(false)));
    let (wake, woken) = mpsc::channel::<()>();
    let s2 = svc.clone();
    let wake2 = Mutex::new(wake);
    if let Err(e) = ipc::serve(move |req| {
        let (resp, enforce) = service::dispatch(&s2, req);
        if enforce {
            let _ = wake2.lock().unwrap().send(());
        }
        resp
    }) {
        util::debug(format!("could not start: {e}"));
        return;
    }
    {
        let s = svc.lock().unwrap();
        crate::autostart::apply(s.state.settings.start_at_login);
    }
    util::debug("service started");
    let tick = Duration::from_millis(std::env::var("PROGRAMPRISON_TICK_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000));
    loop {
        let close = {
            let mut s = svc.lock().unwrap();
            if s.quit_requested && !s.any_locked() {
                break;
            }
            s.check_clock();
            s.enforce(false).apps.close
        };
        // Only look at the process list while a block with programs is on.
        if !close.is_empty() {
            let list = procs::list();
            svc.lock().unwrap().close_programs(&list, &close);
        }
        let _ = woken.recv_timeout(tick);
        while woken.try_recv().is_ok() {}
    }
    let _ = std::fs::remove_file(ipc::endpoint_path());
    let _ = engine::now_ms();
}
