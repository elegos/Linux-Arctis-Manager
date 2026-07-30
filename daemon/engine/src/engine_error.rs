use device_config::api_executor::ApiError;
use device_config::sync_dispatcher::DispatchError;
use device_config::sync_reader::SyncReadError;
use std::fmt;

/// Fatal errors that can occur while running a device session.
#[derive(Debug)]
pub enum EngineError {
    Io(std::io::Error),
    Api(ApiError),
    Dispatch(DispatchError),
    SyncRead(SyncReadError),
    /// Hook name not one of init / post_init / shutdown.
    UnknownLifecycleHook(String),
    /// Lifecycle call name not registered as a built-in.
    UnknownLifecycleCall(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Api(e) => write!(f, "API error: {e}"),
            Self::Dispatch(e) => write!(f, "sync dispatch error: {e}"),
            Self::SyncRead(e) => write!(f, "sync read error: {e}"),
            Self::UnknownLifecycleHook(s) => write!(f, "unknown lifecycle hook '{s}'"),
            Self::UnknownLifecycleCall(s) => write!(f, "unknown lifecycle call '{s}'"),
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Api(e) => Some(e),
            Self::Dispatch(e) => Some(e),
            Self::SyncRead(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn display_variants_are_non_empty() {
        let cases: &[EngineError] = &[
            EngineError::Io(std::io::Error::other("test")),
            EngineError::UnknownLifecycleHook("bad_hook".to_string()),
            EngineError::UnknownLifecycleCall("bad_call".to_string()),
        ];
        for err in cases {
            assert!(!err.to_string().is_empty(), "{err:?} had empty Display");
        }
    }

    #[test]
    fn io_error_source_is_present() {
        let err = EngineError::Io(std::io::Error::other("pipe"));
        assert!(err.source().is_some());
    }

    #[test]
    fn unknown_hook_source_is_none() {
        let err = EngineError::UnknownLifecycleHook("x".to_string());
        assert!(err.source().is_none());
    }
}
