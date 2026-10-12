use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    fmt::{
        FmtContext,
        format::{FormatEvent, FormatFields, Writer},
        time::{ChronoLocal, FormatTime},
    },
    layer::{Layer, SubscriberExt},
    registry::LookupSpan,
    util::SubscriberInitExt,
};

use tracing::{Event, Level, Subscriber};
use tracing_log::NormalizeEvent;

struct ConsoleFormat {
    timer: ChronoLocal,
}

impl<S, N> FormatEvent<S, N> for ConsoleFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(&self, context: &FmtContext<'_, S, N>, mut writer: Writer<'_>, event: &Event<'_>) -> std::fmt::Result {
        use tracing_subscriber::fmt::FormatFields as _;

        let normalized_metadata = event.normalized_metadata();
        let metadata = normalized_metadata.as_ref().unwrap_or_else(|| event.metadata());
        let has_ansi = writer.has_ansi_escapes();

        if has_ansi {
            writer.write_str("\x1b[2m")?;
        }
        if self.timer.format_time(&mut writer).is_err() {
            writer.write_str("<unknown time>")?;
        }
        if has_ansi {
            writer.write_str("\x1b[0m")?;
        }
        writer.write_char(' ')?;
        let (level, color) = match *metadata.level() {
            Level::TRACE => ("TRACE", "\x1b[35m"),
            Level::DEBUG => ("DEBUG", "\x1b[34m"),
            Level::INFO => ("INFO", "\x1b[32m"),
            Level::WARN => ("WARN", "\x1b[33m"),
            Level::ERROR => ("ERROR", "\x1b[31m"),
        };
        if has_ansi {
            writer.write_str(color)?;
        }
        writer.write_str(level)?;
        if has_ansi {
            writer.write_str("\x1b[0m")?;
        }
        writer.write_char(' ')?;

        if has_ansi {
            writer.write_str("\x1b[2m")?;
        }
        write!(&mut writer, "{}:", metadata.target())?;
        if has_ansi {
            writer.write_str("\x1b[0m")?;
        }
        writer.write_char(' ')?;

        context.format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

fn project_filter() -> Targets {
    let level = LevelFilter::TRACE;
    Targets::new()
        .with_default(LevelFilter::INFO)
        .with_target("viewer", level)
        .with_target("network", level)
        .with_target("protocol", level)
        .with_target("llsd", level)
}

pub(crate) fn init() {
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .event_format(ConsoleFormat { timer: ChronoLocal::new("%H:%M:%S%.3f".to_owned()) })
            .with_filter(project_filter()),
    );

    #[cfg(feature = "tracy")]
    let subscriber = subscriber.with(tracing_tracy::TracyLayer::default().with_filter(project_filter()));

    if let Err(error) = subscriber.try_init() {
        eprintln!("Could not initialize tracing: {error}");
    }
}
