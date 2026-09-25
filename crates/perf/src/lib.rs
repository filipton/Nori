//! A perf recorder's bookkeeping (perf_log.rs): the state a stretch of the app's life is filed under,
//! what two readings of the platform's counters make, the stretches kept in the app's database, their
//! sums, the Performance page's figures and the shared report; and the invariant watchdogs
//! (invariants.rs), which say on the same timeline when something that must hold did not.

pub mod invariants;
pub mod perf_log;
