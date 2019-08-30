//! Shared datatypes for azul-* crates

extern crate azul_css;
extern crate gleam;
#[cfg(feature = "css_parser")]
extern crate azul_css_parser;

/// Useful macros for implementing Azul APIs without duplicating code
#[macro_use]
pub mod macros;
/// Functions to manage adding fonts + images, garbage collection
pub mod app_resources;
/// Type definitions for various types of callbacks, as well as focus and scroll handling
pub mod callbacks;
/// Layout and display list creation algorithm, z-index reordering of
pub mod display_list;
/// `Dom` construction, `NodeData` and `NodeType` management functions
pub mod dom;
/// Algorithms to create git-like diffs between two doms in linear time
pub mod diff;
/// Contains OpenGL helper functions (to compile / link shaders), `VirtualGlDriver` for unit testing
pub mod gl;
/// Internal, arena-based storage for Dom nodes
pub mod id_tree;
/// CSS cascading module
pub mod style;
/// Main `Layout` and `GetTextLayout` trait definition
pub mod traits;
/// Async (task, thread, timer) helper functions
pub mod task;
/// `UiDescription` = CSSOM, cascading
pub mod ui_description;
/// Contains functions to build the `Dom`
pub mod ui_state;
pub mod ui_solver;
pub mod window;
pub mod window_state;

// Typedef for possible faster implementation of hashing
pub type FastHashMap<T, U> = ::std::collections::HashMap<T, U>;
pub type FastHashSet<T> = ::std::collections::HashSet<T>;
