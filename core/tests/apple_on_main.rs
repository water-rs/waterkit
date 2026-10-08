//! `on_main` needs a main thread serving the main queue, the way an app's
//! run loop does, so this target owns the process main thread: the harness
//! runs single-threaded, and the test spins the main run loop until the
//! worker that awaits `on_main` stops it.

use libtest_mimic::{Arguments, Trial};

fn main() {
    let mut arguments = Arguments::from_args();
    arguments.test_threads = Some(1);
    #[cfg(target_vendor = "apple")]
    let trials = vec![Trial::test(
        "on_main_dispatches_off_main_and_runs_inline_on_main",
        apple::on_main_dispatches,
    )];
    #[cfg(not(target_vendor = "apple"))]
    let trials: Vec<Trial> = Vec::new();
    libtest_mimic::run(&arguments, trials).exit();
}

#[cfg(target_vendor = "apple")]
mod apple {
    use libtest_mimic::Failed;

    pub fn on_main_dispatches() -> Result<(), Failed> {
        use objc2_core_foundation::CFRunLoop;
        use waterkit_core::apple::on_main;

        let worker = std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(|| {
                let caller = std::thread::current().id();
                let (ran_elsewhere, value) = futures::executor::block_on(on_main(move |_mtm| {
                    (std::thread::current().id() != caller, 7)
                }));
                // On the main thread `on_main` runs inline.
                let inline = futures::executor::block_on(on_main(|_mtm| {
                    futures::executor::block_on(on_main(|_mtm| 9))
                }));
                (ran_elsewhere, value, inline)
            });
            // The run loop is not `Send`; the main queue stops it from its own
            // thread.
            dispatch2::DispatchQueue::main().exec_async(|| {
                CFRunLoop::current()
                    .expect("the main thread has a run loop")
                    .stop();
            });
            outcome
        });
        CFRunLoop::run();
        let (ran_elsewhere, value, inline) = worker
            .join()
            .expect("the worker thread catches its own panics")
            .map_err(|_| Failed::from("on_main panicked"))?;
        if !ran_elsewhere {
            return Err("work dispatched from a worker ran on that worker".into());
        }
        if value != 7 || inline != 9 {
            return Err(format!("on_main returned {value} and {inline}, expected 7 and 9").into());
        }
        Ok(())
    }
}
