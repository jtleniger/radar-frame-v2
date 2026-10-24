//! Process lifecycle: startup validation, the clock, the render facade, the
//! published frame, the update cycle, and the on-disk setup artifacts.

pub mod clock;
pub mod frame;
pub mod modes;
pub mod renderer;
pub mod setup;
pub mod startup;
