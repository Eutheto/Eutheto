//! People CSV contracts for bounded, explicit review outside application authority.

mod mapping;
mod parsing;
mod review;
mod types;

pub use parsing::detect_people_csv;
pub use review::*;
pub use types::*;
