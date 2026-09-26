//! One thread for the core's own background work - database writes the platform used to schedule on
//! its I/O threads (a play recorded in the history, and the like) - so the caller, often the main or
//! the audio thread, never waits on the database, and the platform does not have to hop threads first.

use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;

use parking_lot::Mutex;

type Job = Box<dyn FnOnce() + Send>;

fn sender() -> &'static Mutex<Sender<Job>> {
    static TX: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        std::thread::Builder::new()
            .name("nori-core".into())
            .spawn(move || {
                for job in rx {
                    job();
                }
            })
            .expect("the core's background thread starts");
        Mutex::new(tx)
    })
}

/// Runs `job` on the core's background thread, after whatever was handed over before it.
pub fn run(job: impl FnOnce() + Send + 'static) {
    let _ = sender().lock().send(Box::new(job));
}
