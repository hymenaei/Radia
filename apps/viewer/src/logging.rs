use std::{
    backtrace::Backtrace,
    env,
    fs::{self, OpenOptions},
    io::{self, IsTerminal},
    panic,
    path::Path,
    thread,
};

use time::OffsetDateTime;
use tracing_appender::non_blocking::{NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::{
    Layer,
    filter::{LevelFilter, Targets},
    fmt::{self, time::ChronoLocal},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

const CRATES: [&str; 11] = ["viewer", "assets", "base", "llsd", "testing", "math", "network", "protocol", "render", "scene", "world"];

#[must_use]
pub(crate) fn init() -> Option<WorkerGuard> {
    let log_filter = match env::var("LOG") {
        Ok(filter) => match filter.parse::<Targets>() {
            Ok(filter) => Some(filter),
            Err(error) => {
                eprintln!("Ignoring invalid LOG filter: {error}");
                None
            }
        },
        Err(_) => None,
    };

    let console_filter = log_filter
        .clone()
        .unwrap_or_else(|| Targets::new().with_default(LevelFilter::WARN).with_targets(CRATES.map(|name| (name, LevelFilter::INFO))));
    let file_filter = log_filter
        .unwrap_or_else(|| Targets::new().with_default(LevelFilter::WARN).with_targets(CRATES.map(|name| (name, LevelFilter::TRACE))));

    let now = OffsetDateTime::now_utc();
    let name = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}.jsonl",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let dir = Path::new("target/logs");
    let (writer, guard) = match fs::create_dir_all(dir).and_then(|()| OpenOptions::new().write(true).create_new(true).open(dir.join(name)))
    {
        Ok(file) => {
            let (writer, guard) = NonBlockingBuilder::default().buffered_lines_limit(16_384).lossy(true).finish(file);
            (Some(writer), Some(guard))
        }
        Err(error) => {
            eprintln!("Could not create session log: {error}");
            (None, None)
        }
    };

    let console = fmt::layer()
        .compact()
        .with_timer(ChronoLocal::new("%H:%M:%S%.3f".into()))
        .with_ansi(io::stderr().is_terminal())
        .with_target(true)
        .with_writer(io::stderr)
        .with_filter(console_filter);

    let json = writer.map(|writer| {
        fmt::layer()
            .json()
            .with_writer(writer)
            .with_file(true)
            .with_line_number(true)
            .with_thread_names(true)
            .with_current_span(false)
            .with_span_list(true)
            .with_filter(file_filter)
    });

    let subscriber = tracing_subscriber::registry().with(console).with(json);

    #[cfg(feature = "tracy")]
    let subscriber = subscriber
        .with(tracing_tracy::TracyLayer::default().with_filter(Targets::new().with_targets(CRATES.map(|name| (name, LevelFilter::TRACE)))));

    if let Err(error) = subscriber.try_init() {
        eprintln!("Could not initialize tracing: {error}");
    }

    let default_hook = panic::take_hook();

    panic::set_hook(Box::new(move |info| {
        let current = thread::current();

        tracing::error!(
            target: "viewer::panic",
            panic = info.payload_as_str().unwrap_or("non-string panic payload"),
            thread = current.name().unwrap_or("<unnamed>"),
            location = info.location().map(tracing::field::display),
            backtrace = %Backtrace::force_capture(),
            "panic"
        );

        default_hook(info);
    }));

    guard
}
