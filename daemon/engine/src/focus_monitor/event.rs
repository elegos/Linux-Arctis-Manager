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
}
