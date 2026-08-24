/// Events emitted by each focus-tracking backend.
/// Every backend implementation must send exactly these variants.
pub enum FocusEvent {
    /// A window gained focus.  `pid` is the owning process; `class` is the
    /// X11/Wayland window class or app-id (used as fallback when PID is unavailable).
    Focused {
        pid: Option<u32>,
        class: Option<String>,
    },
    /// A window was closed.
    Closed { pid: u32 },
    /// A synthetic X11 window gained focus — a Wayland-native app (no XWayland mapping).
    /// `xwayland_pids` lists PIDs that own real X11 windows; the focused process is not
    /// among them.  The receiver must scan /proc to find the matching override.
    WaylandNativeFocused { xwayland_pids: Vec<u32> },
}
