//! The output side: the platform's output devices mapped onto the player's and the ones known so far
//! (outputs.rs), which sound each device gets as whole steps (profiles.rs), and the AutoEQ headphone
//! database kept on the device (autoeq.rs). The steps' calls on the profiles in the core's database are the
//! core's.

pub mod autoeq;
pub mod outputs;
pub mod profiles;
