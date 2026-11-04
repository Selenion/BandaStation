//! Optional diagnostic CPU counters. Thread CPU excludes time suspended in the await notification.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Default)]
pub(super) struct Profile {
    cpu_ns: AtomicU64,
    jobs: AtomicU64,
    clock_available: AtomicBool,
    cycles: AtomicU64,
    cycles_available: AtomicBool,
}

impl Profile {
    pub(super) fn record(&self, start: Option<u64>, cycle_start: Option<u64>) {
        if let (Some(start), Some(end)) = (start, thread_cpu_ns()) {
            self.cpu_ns
                .fetch_add(end.saturating_sub(start), Ordering::Relaxed);
            self.clock_available.store(true, Ordering::Relaxed);
        }
        if let (Some(start), Some(end)) = (cycle_start, thread_cycles()) {
            self.cycles
                .fetch_add(end.saturating_sub(start), Ordering::Relaxed);
            self.cycles_available.store(true, Ordering::Relaxed);
        }
        self.jobs.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn read(&self) -> [f32; 5] {
        [
            self.cpu_ns.load(Ordering::Relaxed) as f32 / 1e9,
            self.jobs.load(Ordering::Relaxed) as f32,
            u8::from(self.clock_available.load(Ordering::Relaxed)) as f32,
            u8::from(super::super::simd_enabled()) as f32,
            if self.cycles_available.load(Ordering::Relaxed) {
                self.cycles.load(Ordering::Relaxed) as f32 / 1e6
            } else {
                -1.0
            },
        ]
    }
}

#[cfg(windows)]
pub(super) fn thread_cycles() -> Option<u64> {
    use std::ffi::c_void;
    extern "system" {
        fn GetCurrentThread() -> *mut c_void;
        fn QueryThreadCycleTime(thread: *mut c_void, cycles: *mut u64) -> i32;
    }
    let mut cycles = 0;
    let success = unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) };
    // Cycles are kept in their native unit; converting by a nominal CPU frequency is invalid.
    (success != 0).then_some(cycles)
}

#[cfg(not(windows))]
pub(super) fn thread_cycles() -> Option<u64> {
    None
}

#[cfg(windows)]
pub(super) fn thread_cpu_ns() -> Option<u64> {
    use std::ffi::c_void;
    extern "system" {
        fn GetCurrentThread() -> *mut c_void;
        fn GetThreadTimes(
            thread: *mut c_void,
            created: *mut u64,
            exited: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
    }
    let mut times = [0_u64; 4];
    let [created, exited, kernel, user] = &mut times;
    // GetCurrentThread returns a borrowed pseudo-handle. FILETIME is a little-endian pair of u32.
    let success = unsafe { GetThreadTimes(GetCurrentThread(), created, exited, kernel, user) };
    (success != 0).then(|| (times[2] + times[3]) * 100)
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(super) fn thread_cpu_ns() -> Option<u64> {
    #[repr(C)]
    struct Time {
        seconds: std::ffi::c_long,
        nanos: std::ffi::c_long,
    }
    extern "C" {
        fn clock_gettime(clock: i32, output: *mut Time) -> i32;
    }
    let mut time = Time {
        seconds: 0,
        nanos: 0,
    };
    // Linux CLOCK_THREAD_CPUTIME_ID, with the target's native timespec layout.
    let result = unsafe { clock_gettime(3, &mut time) };
    (result == 0).then(|| time.seconds as u64 * 1_000_000_000 + time.nanos as u64)
}

#[cfg(not(any(windows, all(target_os = "linux", target_env = "gnu"))))]
pub(super) fn thread_cpu_ns() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_counts_cpu_work_instead_of_time_asleep() {
        let Some(before) = thread_cpu_ns() else {
            return;
        };
        std::thread::sleep(std::time::Duration::from_millis(40));
        let after_sleep = thread_cpu_ns().unwrap();
        let mut number = 220_u64;
        for _ in 0..50_000_000 {
            number = std::hint::black_box(number)
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1);
        }
        std::hint::black_box(number);
        let after_work = thread_cpu_ns().unwrap();
        assert!(after_work > after_sleep);
        assert!(after_sleep - before < 30_000_000);
    }
}
