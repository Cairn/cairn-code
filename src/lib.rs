pub mod agent;
pub mod config;
pub mod cost;
pub mod http_client;
pub mod json;
pub mod llm;
pub mod markdown;
pub mod notify;
pub mod oauth;
pub mod redact;
pub mod session;
pub mod theme;
pub mod tools;
pub mod tui;

#[cfg(test)]
pub(crate) mod test_support {
    use std::ffi::OsString;
    use std::sync::Mutex;

    pub static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub struct EnvGuard {
        name: &'static str,
        value: Option<OsString>,
    }

    impl EnvGuard {
        pub fn capture(name: &'static str) -> Self {
            Self {
                name,
                value: std::env::var_os(name),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.value {
                std::env::set_var(self.name, value);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }
}
