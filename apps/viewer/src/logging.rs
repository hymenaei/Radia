use std::{
    backtrace::Backtrace,
    fs::{self, OpenOptions},
    io::{self, IsTerminal},
    panic,
    path::Path,
};

use chrono::{Duration, Local};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter,
    fmt::{self, time::ChronoLocal},
    prelude::*,
};

const LOG_DIR: &str = "target/logs";
const MAX_LOGS: usize = 20;

pub(crate) fn init() -> Option<WorkerGuard> {
    let (json, guard, file_error) = match create_session_log() {
        Ok(file) => {
            let (writer, guard) = tracing_appender::non_blocking(file);
            let file_filter = EnvFilter::try_from_env("FILE_LOG").unwrap_or_else(|_| EnvFilter::new("trace"));
            let json = fmt::layer()
                .json()
                .with_timer(ChronoLocal::new("%Y-%m-%dT%H:%M:%S%.3f%:z".into()))
                .with_file(true)
                .with_line_number(true)
                .with_writer(writer)
                .with_filter(file_filter);
            (Some(json), Some(guard), None)
        }
        Err(error) => (None, None, Some(error)),
    };

    let console_filter = EnvFilter::try_from_env("LOG").unwrap_or_else(|_| EnvFilter::new("warn,viewer=info"));

    let console = fmt::layer()
        .compact()
        .with_timer(ChronoLocal::new("%H:%M:%S%.3f".into()))
        .with_ansi(io::stderr().is_terminal())
        .with_writer(io::stderr)
        .with_filter(console_filter);

    let subscriber = tracing_subscriber::registry().with(console).with(json);

    #[cfg(feature = "tracy")]
    let subscriber = subscriber.with(tracing_tracy::TracyLayer::default());

    if let Err(error) = subscriber.try_init() {
        eprintln!("Could not initialize tracing: {error}");
    }

    if let Some(error) = file_error {
        tracing::warn!(%error, "Could not create session log; continuing with console logging");
    }

    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        tracing::error!(
            target: "viewer::panic",
            panic = info.payload_as_str().unwrap_or("non-string panic payload"),
            location = info.location().map(tracing::field::display),
            backtrace = %Backtrace::force_capture(),
            "panic"
        );
        default_hook(info);
    }));

    guard
}

fn create_session_log() -> io::Result<std::fs::File> {
    let dir = Path::new(LOG_DIR);
    fs::create_dir_all(dir)?;

    let mut time = Local::now();

    let (file, path) = loop {
        let path = dir.join(format!("{}.jsonl", time.format("%Y%m%d-%H%M%S")));

        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (file, path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                time += Duration::seconds(1);
            }
            Err(error) => return Err(error),
        }
    };

    let latest = dir.join("latest.jsonl");

    match fs::remove_file(&latest) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    fs::hard_link(&path, &latest)?;
    prune_logs(dir)?;

    Ok(file)
}

fn prune_logs(dir: &Path) -> io::Result<()> {
    let mut logs = fs::read_dir(dir)?.map(|entry| entry.map(|entry| entry.path())).collect::<io::Result<Vec<_>>>()?;

    logs.retain(|path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        let Some(stamp) = name.strip_suffix(".jsonl") else {
            return false;
        };

        let bytes = stamp.as_bytes();
        bytes.len() == 15 && bytes[8] == b'-' && bytes[..8].iter().all(u8::is_ascii_digit) && bytes[9..].iter().all(u8::is_ascii_digit)
    });

    logs.sort();

    let excess = logs.len().saturating_sub(MAX_LOGS);
    for path in logs.into_iter().take(excess) {
        fs::remove_file(path)?;
    }

    Ok(())
}
