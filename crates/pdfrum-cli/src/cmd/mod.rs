//! One module per command family.

pub mod diff;
pub mod doctor;
pub mod extract;
pub mod file;
pub mod forms;
pub mod hash;
pub mod info;
pub mod inspect;
pub mod pages;
pub mod render;
#[cfg(feature = "javascript")]
pub mod scripts;
pub mod serve;
pub mod shell;
pub mod terminal;
