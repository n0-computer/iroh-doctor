//! Tracing setup: stdout, an optional daily-rolling file in the config
//! dir, and (on iOS) a forward into the unified log.

use std::path::PathBuf;

use crate::identity;

/// Returns the directory we write rolling log files to. `None` when no
/// config dir is available on the platform; the app then logs only to
/// stdout.
pub fn log_dir() -> Option<PathBuf> {
    let dir = identity::config_dir().ok()?.join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Sets up stdout + (optional) daily rolling file logging. Returns the
/// `WorkerGuard` for the file writer; dropping it would stop flushing
/// log lines, so `main()` keeps it bound for the program lifetime.
pub fn init(log_dir: Option<&PathBuf>) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,iroh_doctor_app=debug".into());

    let stdout_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stdout);

    let (file_layer, guard) = match log_dir {
        Some(dir) => {
            let file_appender = tracing_appender::rolling::daily(dir, "iroh-doctor-app.log");
            let (writer, guard) = tracing_appender::non_blocking(file_appender);
            (
                Some(
                    tracing_subscriber::fmt::layer()
                        .with_writer(writer)
                        .with_ansi(false),
                ),
                Some(guard),
            )
        }
        None => (None, None),
    };

    // On iOS, stdout is dropped (GUI apps aren't attached to a terminal) and
    // the rolling file lives inside the app sandbox, so neither layer above is
    // visible to `log stream` / Console.app / Xcode. Forward tracing into
    // `os_log` so iroh's logs (discovery publish, relay, magicsock, ...) land
    // in the iOS unified log, filterable by `subsystem:com.number0.iroh-doctor-app`.
    // `Option<L>` implements `Layer`, so the non-iOS no-op (`None`) keeps the
    // registry type identical across targets.
    #[cfg(target_os = "ios")]
    let oslog_layer = Some(tracing_oslog::OsLogger::new(
        "com.number0.iroh-doctor-app",
        "default",
    ));
    #[cfg(not(target_os = "ios"))]
    let oslog_layer: Option<tracing_subscriber::layer::Identity> = None;

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer)
        .with(oslog_layer)
        .init();
    guard
}
