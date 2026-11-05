//! emu_coverage: play libretro input-replay trees, check state hashes and measure ROM coverage.
//! The `emucov` command-line tool and the GUI are both built on this library.

pub mod analysis;
pub mod collection;
pub mod coverage;
pub mod format;
pub mod libretro;
pub mod player;
