use std::collections::HashMap;
use std::fmt;

use serde_yaml::Value as Yaml;

use crate::{DeviceConfig, Lifecycle};

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum LifecycleError {
    /// Caller passed a name that is not one of `init`, `post_init`, `shutdown`.
    UnknownHook(String),
    /// A call in the hook sequence has no registered handler.
    UnknownCall(String),
    /// A registered handler returned an error.
    CallFailed { call: String, reason: String },
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownHook(s) => write!(f, "unknown lifecycle hook: '{s}'"),
            Self::UnknownCall(s) => write!(f, "unregistered lifecycle call: '{s}'"),
            Self::CallFailed { call, reason } => {
                write!(f, "lifecycle call '{call}' failed: {reason}")
            }
        }
    }
}

impl std::error::Error for LifecycleError {}

// ── Handler type ──────────────────────────────────────────────────────────────

/// A registered lifecycle handler.  Receives the call's optional `args` value
/// and returns `Ok(())` on success or an error message on failure.
pub type LifecycleFn = Box<dyn Fn(Option<&Yaml>) -> Result<(), String> + Send + Sync>;

// ── Executor ──────────────────────────────────────────────────────────────────

/// Executes named lifecycle hooks by dispatching each call to a registered
/// handler.  Handlers are registered by the engine at startup and may perform
/// I/O; the executor itself is I/O-free (it only drives the dispatch loop).
pub struct LifecycleExecutor<'a> {
    lifecycle: Option<&'a Lifecycle>,
    handlers: HashMap<String, LifecycleFn>,
}

impl<'a> LifecycleExecutor<'a> {
    pub fn new(cfg: &'a DeviceConfig) -> Self {
        Self {
            lifecycle: cfg.lifecycle.as_ref(),
            handlers: HashMap::new(),
        }
    }

    /// Register a handler for a named lifecycle call (e.g. `"save_to_flash"`).
    /// Must be called before `run` for any call that appears in the config.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(Option<&Yaml>) -> Result<(), String> + Send + Sync + 'static,
    ) {
        self.handlers.insert(name.into(), Box::new(f));
    }

    /// Execute all calls in the named hook (`init`, `post_init`, or `shutdown`)
    /// sequentially.  Returns `Ok(())` if the hook is absent or has no calls.
    /// Returns `Err(UnknownHook)` if `hook` is not one of the three valid names.
    pub fn run(&self, hook: &str) -> Result<(), LifecycleError> {
        let calls = self.hook_calls(hook)?;
        for lc in calls {
            let handler = self
                .handlers
                .get(&lc.call)
                .ok_or_else(|| LifecycleError::UnknownCall(lc.call.clone()))?;
            handler(lc.args.as_ref()).map_err(|reason| LifecycleError::CallFailed {
                call: lc.call.clone(),
                reason,
            })?;
        }
        Ok(())
    }

    fn hook_calls(&self, hook: &str) -> Result<&[crate::LifecycleCall], LifecycleError> {
        let lc = match self.lifecycle {
            Some(lc) => lc,
            None => {
                return match hook {
                    "init" | "post_init" | "shutdown" => Ok(&[]),
                    _ => Err(LifecycleError::UnknownHook(hook.to_string())),
                };
            }
        };
        match hook {
            "init" => Ok(lc.init.as_deref().unwrap_or(&[])),
            "post_init" => Ok(lc.post_init.as_deref().unwrap_or(&[])),
            "shutdown" => Ok(lc.shutdown.as_deref().unwrap_or(&[])),
            _ => Err(LifecycleError::UnknownHook(hook.to_string())),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn cfg(yaml: &str) -> DeviceConfig {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn run_init_executes_calls_in_order() {
        let c = cfg(r#"
lifecycle:
  init:
    - call: step_a
    - call: step_b
    - call: step_c
"#);
        let mut ex = LifecycleExecutor::new(&c);
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        for name in ["step_a", "step_b", "step_c"] {
            let log = Arc::clone(&log);
            let name = name.to_string();
            ex.register(name.clone(), move |_| {
                log.lock().unwrap().push(name.clone());
                Ok(())
            });
        }

        ex.run("init").unwrap();
        assert_eq!(*log.lock().unwrap(), vec!["step_a", "step_b", "step_c"]);
    }

    #[test]
    fn run_absent_hook_is_noop() {
        // Config only has shutdown; running init or post_init should succeed silently.
        let c = cfg(r#"
lifecycle:
  shutdown:
    - call: save_to_flash
"#);
        let mut ex = LifecycleExecutor::new(&c);
        ex.register("save_to_flash", |_| Ok(()));
        ex.run("init").unwrap();
        ex.run("post_init").unwrap();
    }

    #[test]
    fn run_no_lifecycle_in_config_is_noop() {
        let c = cfg("{}");
        let ex = LifecycleExecutor::new(&c);
        ex.run("init").unwrap();
        ex.run("post_init").unwrap();
        ex.run("shutdown").unwrap();
    }

    #[test]
    fn run_unknown_hook_name_errors() {
        let c = cfg("{}");
        let ex = LifecycleExecutor::new(&c);
        assert!(matches!(
            ex.run("flurp").unwrap_err(),
            LifecycleError::UnknownHook(s) if s == "flurp"
        ));
    }

    #[test]
    fn run_unregistered_call_errors() {
        let c = cfg(r#"
lifecycle:
  init:
    - call: enable_sonar
"#);
        let ex = LifecycleExecutor::new(&c);
        assert!(matches!(
            ex.run("init").unwrap_err(),
            LifecycleError::UnknownCall(s) if s == "enable_sonar"
        ));
    }

    #[test]
    fn run_call_failure_propagates() {
        let c = cfg(r#"
lifecycle:
  shutdown:
    - call: save_to_flash
"#);
        let mut ex = LifecycleExecutor::new(&c);
        ex.register("save_to_flash", |_| Err("disk full".to_string()));
        assert!(matches!(
            ex.run("shutdown").unwrap_err(),
            LifecycleError::CallFailed { call, reason }
                if call == "save_to_flash" && reason == "disk full"
        ));
    }

    #[test]
    fn run_stops_on_first_failure() {
        let c = cfg(r#"
lifecycle:
  init:
    - call: step_a
    - call: step_b
"#);
        let mut ex = LifecycleExecutor::new(&c);
        let ran_b = Arc::new(Mutex::new(false));
        let ran_b2 = Arc::clone(&ran_b);

        ex.register("step_a", |_| Err("boom".to_string()));
        ex.register("step_b", move |_| {
            *ran_b2.lock().unwrap() = true;
            Ok(())
        });

        assert!(ex.run("init").is_err());
        assert!(
            !*ran_b.lock().unwrap(),
            "step_b should not have run after step_a failed"
        );
    }

    #[test]
    fn run_args_passed_to_handler() {
        let c = cfg(r#"
lifecycle:
  init:
    - call: discord_certified_set_attributes
      args: {echo_cancellation: true, noise_suppression: true}
"#);
        let mut ex = LifecycleExecutor::new(&c);
        let received: Arc<Mutex<Option<Yaml>>> = Arc::new(Mutex::new(None));
        let received2 = Arc::clone(&received);

        ex.register("discord_certified_set_attributes", move |args| {
            *received2.lock().unwrap() = args.cloned();
            Ok(())
        });

        ex.run("init").unwrap();

        let got = received.lock().unwrap();
        let got = got.as_ref().expect("args should have been passed");
        assert_eq!(got["echo_cancellation"], Yaml::Bool(true));
        assert_eq!(got["noise_suppression"], Yaml::Bool(true));
    }

    #[test]
    fn run_call_with_no_args_passes_none() {
        let c = cfg(r#"
lifecycle:
  post_init:
    - call: sync_all
"#);
        let mut ex = LifecycleExecutor::new(&c);
        let args_was_none = Arc::new(Mutex::new(false));
        let flag = Arc::clone(&args_was_none);

        ex.register("sync_all", move |args| {
            *flag.lock().unwrap() = args.is_none();
            Ok(())
        });

        ex.run("post_init").unwrap();
        assert!(*args_was_none.lock().unwrap());
    }

    #[test]
    fn run_shutdown_hook() {
        let c = cfg(r#"
lifecycle:
  shutdown:
    - call: disable_chatmix
    - call: disable_sonar
    - call: save_to_flash
"#);
        let mut ex = LifecycleExecutor::new(&c);
        let count = Arc::new(Mutex::new(0u32));
        for name in ["disable_chatmix", "disable_sonar", "save_to_flash"] {
            let count = Arc::clone(&count);
            ex.register(name, move |_| {
                *count.lock().unwrap() += 1;
                Ok(())
            });
        }
        ex.run("shutdown").unwrap();
        assert_eq!(*count.lock().unwrap(), 3);
    }
}
