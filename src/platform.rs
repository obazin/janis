//! What Janis asks of the operating system, resolved the same way everywhere.

use std::path::PathBuf;

/// The application's identifier, which names its per-user data directory.
pub const APPLICATION_IDENTIFIER: &str = "com.olivierbazin.janis";

/// The directory `janis.db` lives in: the per-user data directory of the
/// platform, under the application's identifier.
pub fn data_directory() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|directory| directory.join(APPLICATION_IDENTIFIER))
        .ok_or_else(|| "this system reports no per-user data directory".to_owned())
}

/// True where the system draws its window buttons over the application's own
/// title bar, which then leaves room for them.
pub const OVERLAY_TITLE_BAR: bool = cfg!(target_os = "macos");

/// Selects the windowing backend, asking for a title bar the application
/// draws itself wherever the system offers one that keeps its window buttons.
pub fn select_backend() -> Result<(), String> {
    let selector = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg".into());
    #[cfg(target_os = "macos")]
    let selector = {
        use slint::winit_030::winit::platform::macos::WindowAttributesExtMacOS;
        selector.with_winit_window_attributes_hook(|attributes| {
            attributes
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true)
        })
    };
    selector
        .select()
        .map_err(|error| format!("select the windowing backend: {error}"))
}

/// Brings the window in front of the others and gives it the keyboard.
pub fn bring_forward(window: &slint::Window) {
    use slint::winit_030::WinitWindowAccessor;
    window.with_winit_window(|winit_window| {
        winit_window.set_minimized(false);
        winit_window.focus_window();
    });
}

/// Calls `dropped` for each file the user drops on the window.
pub fn on_file_dropped(window: &slint::Window, mut dropped: impl FnMut(PathBuf) + 'static) {
    use slint::winit_030::winit::event::WindowEvent;
    use slint::winit_030::{EventResult, WinitWindowAccessor};
    window.on_winit_window_event(move |_, event| {
        if let WindowEvent::DroppedFile(path) = event {
            dropped(path.clone());
        }
        EventResult::Propagate
    });
}

/// Starts moving the window with the pointer, for a title bar the application
/// draws itself.
pub fn drag_window(window: &slint::Window) {
    use slint::winit_030::WinitWindowAccessor;
    window.with_winit_window(|winit_window| {
        if let Err(error) = winit_window.drag_window() {
            log::warn!("start window drag: {error}");
        }
    });
}
