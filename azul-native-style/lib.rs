//! Provides azul-compatible approximations of OS-native styles.

extern crate azul_css;
extern crate azul_css_parser;

use azul_css::Css;

/// CSS mimicking the OS-native look - Windows: `styles/native_windows.css`
pub const WINDOWS_CSS: &str = concat!(
    include_str!("styles/native_windows.css"),
    include_str!("styles/shared/table.css"),
);

/// CSS mimicking the OS-native look - Linux: `styles/native_linux.css`
pub const LINUX_CSS: &str = concat!(
    include_str!("styles/native_linux.css"),
    include_str!("styles/shared/table.css"),
);

/// CSS mimicking the OS-native look - Mac: `styles/native_macos.css`
pub const MACOS_CSS: &str = concat!(
    include_str!("styles/native_macos.css"),
    include_str!("styles/shared/table.css"),
);

#[cfg(target_os="windows")]
pub const NATIVE_CSS: &str = WINDOWS_CSS;
#[cfg(target_os="macos")]
pub const NATIVE_CSS: &str = MACOS_CSS;
#[cfg(target_os="linux")]
pub const NATIVE_CSS: &str = LINUX_CSS;

/// Returns the native style for the OS
///
/// TODO: Use OS version / load system style here!
pub fn native() -> Css {
    azul_css_parser::new_from_str(NATIVE_CSS).unwrap()
}
