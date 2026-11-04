//! One in-flight calculation per owner, without CPU affinity or a worker per Z level.
use super::*;
use std::{
    ops::Deref,
    sync::{Condvar, MutexGuard},
};
#[cfg(feature = "worker-profile")]
#[path = "worker_profile.rs"]
mod profile;

#[derive(Default)]
pub(super) struct Engine {
    store: Mutex<Store>,
    ready: Condvar,
    #[cfg(feature = "worker-profile")]
    profile: profile::Profile,
}

impl Deref for Engine {
    type Target = Mutex<Store>;

    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl Engine {
    #[cfg(feature = "worker-profile")]
    pub(super) fn profile(&self) -> [f32; 5] {
        self.profile.read()
    }
    pub(super) fn notify(&self) {
        self.ready.notify_all();
    }

    pub(super) fn publish(
        &self,
        mut store: MutexGuard<'_, Store>,
        cycle: u32,
    ) -> Result<(), String> {
        while store.pending_cycle() == Some(cycle) && !store.ready() {
            store = self.ready.wait(store).map_err(|_| "Phase owner poisoned")?;
        }
        store.publish_ready(cycle).map_err(|e| {
            format!(
                "{e:?}: requested cycle {cycle}, pending {:?}, published {}, ready {}",
                store.pending_cycle(),
                store.snapshot().epoch,
                store.ready()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishing_does_not_wait_for_the_workers_completion_callback() {
        let owner = Arc::new(Engine::default());
        let job = owner.lock().unwrap().begin_at(37).unwrap();
        let worker_owner = owner.clone();
        let main_thread = std::thread::current().id();
        let (ready, received) = std::sync::mpsc::channel();
        let (resume, wait) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let completed = job.calculate();
            worker_owner.lock().unwrap().stage(completed).unwrap();
            worker_owner.notify();
            ready.send(std::thread::current().id()).unwrap();
            // Like Byond_Return, the completion can wait for the main thread after staging.
            wait.recv().unwrap();
        });
        owner.publish(owner.lock().unwrap(), 37).unwrap();
        assert_ne!(received.recv().unwrap(), main_thread);
        assert_eq!(owner.lock().unwrap().snapshot().epoch, 37);
        resume.send(()).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn abort_wakes_a_consumer_waiting_for_unpublished_work() {
        let owner = Arc::new(Engine::default());
        let _job = owner.lock().unwrap().begin_at(2).unwrap();
        let consumer = owner.clone();
        let waiter = std::thread::spawn(move || consumer.publish(consumer.lock().unwrap(), 2));
        owner.lock().unwrap().abort();
        owner.notify();
        assert!(waiter.join().unwrap().unwrap_err().starts_with("StaleJob"));
    }
}

pub(super) unsafe fn start(count: u32, values: *const Value, waiting: Value, staged: bool) {
    let Ok(api) = api() else {
        return;
    };
    let prepared = catch_unwind(AssertUnwindSafe(|| -> Result<_, String> {
        if count != if staged { 2 } else { 1 } || values.is_null() {
            return Err("Invalid tick arguments".into());
        }
        let owner = engine(api.integer(&*values)?)?;
        let job = {
            let mut store = owner.lock().map_err(|_| "Phase owner poisoned")?;
            if staged {
                store.begin_at(api.integer(&*values.add(1))?)
            } else {
                store.begin()
            }
            .map_err(|e| format!("{e:?}"))?
        };
        Ok((owner, job))
    }))
    .unwrap_or_else(|_| Err("Panic preparing atmosphere worker".into()));
    let (owner, job) = match prepared {
        Ok(value) => value,
        Err(error) => {
            remember_error(error);
            (api.complete)(&waiting, &api.number(0.0));
            return;
        }
    };
    (api.inc)(&waiting);
    let failed_spawn_owner = owner.clone();
    let spawned = std::thread::Builder::new()
        .name("banda-atmos".into())
        .spawn(move || {
            #[cfg(feature = "worker-profile")]
            let cpu_started = profile::thread_cpu_ns();
            #[cfg(feature = "worker-profile")]
            let cycles_started = profile::thread_cycles();
            let calculated = catch_unwind(AssertUnwindSafe(|| job.calculate()));
            let result = match owner.lock() {
                Ok(mut store) => match calculated {
                    Ok(completed) => if staged {
                        store.stage(completed)
                    } else {
                        store.finish(completed)
                    }
                    .map_err(|e| format!("{e:?}")),
                    Err(_) => {
                        store.abort();
                        Err("Panic calculating atmosphere worker".into())
                    }
                },
                Err(_) => Err("Phase owner poisoned".into()),
            };
            let success = result.is_ok();
            if let Err(error) = result {
                remember_error(error);
            }
            #[cfg(feature = "worker-profile")]
            owner.profile.record(cpu_started, cycles_started);
            // A synchronous gas consumer may be waiting on this condition. Never join the worker:
            // Byond_Return can itself wait for the DM thread to service its completion notification.
            owner.notify();
            unsafe {
                (api.complete)(&waiting, &api.number(if success { 1.0 } else { 0.0 }));
                (api.dec)(&waiting);
            }
        });
    if spawned.is_err() {
        remember_error("Could not spawn atmosphere worker".into());
        if let Ok(mut store) = failed_spawn_owner.lock() {
            store.abort();
        }
        failed_spawn_owner.notify();
        (api.complete)(&waiting, &api.number(0.0));
        (api.dec)(&waiting);
    }
}

fn remember_error(error: String) {
    eprintln!("Banda atmosphere worker: {error}");
    if let (Ok(error), Ok(mut saved)) = (CString::new(error), last_error().lock()) {
        *saved = error;
    }
}
