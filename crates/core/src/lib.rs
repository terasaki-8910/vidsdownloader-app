//! Core logic of vidsdownloader. No UI and no Tauri dependency.

// Unsafe code is only allowed in `runner`'s platform layer (Windows Job Objects,
// Unix process groups), each block with a SAFETY comment.
#![deny(unsafe_code)]

pub mod args;
pub mod comment_files;
pub mod comments;
pub mod cookie_jar;
pub mod executor;
pub mod filters;
pub mod firefox;
pub mod gallery_dl;
pub mod item_files;
pub mod item_meta;
pub mod naming;
pub mod niconico_guest;
pub mod page_gate;
pub mod page_images;
pub mod page_media;
pub mod parse;
pub mod queue;
pub mod runner;
pub mod saved;
pub mod site_probe;
pub mod store;
pub mod tools;
pub mod types;
