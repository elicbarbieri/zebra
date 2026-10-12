//! Stdout log format under systemd-journald (`$JOURNAL_STREAM`, see systemd.exec(5))
//!
//! - no timestamp (journald stamps every line)
//! - `<N>` syslog priority prefix per line (journald `SyslogLevelPrefix=`, sd-daemon(3))

use std::fmt;

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{
    fmt::{
        format::{self, Writer},
        FmtContext, FormatEvent, FormatFields,
    },
    registry::LookupSpan,
};

pub(super) enum EventFormat {
    Default(format::Format),
    Journal(format::Format<format::Full, ()>),
}

impl EventFormat {
    pub(super) fn detect(logs_to_stdout: bool) -> Self {
        if logs_to_stdout && stdout_is_journal() {
            Self::Journal(format::Format::default().without_time())
        } else {
            Self::Default(format::Format::default())
        }
    }
}

impl<S, N> FormatEvent<S, N> for EventFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        match self {
            Self::Default(format) => format.format_event(ctx, writer, event),
            Self::Journal(format) => {
                let priority = match *event.metadata().level() {
                    Level::ERROR => 3,
                    Level::WARN => 4,
                    Level::INFO => 6,
                    Level::DEBUG | Level::TRACE => 7,
                };
                write!(writer, "<{priority}>")?;
                format.format_event(ctx, writer, event)
            }
        }
    }
}

/// `$JOURNAL_STREAM` = `<dev>:<ino>` of stdout (unset or stale when stdout redirected elsewhere)
#[cfg(unix)]
fn stdout_is_journal() -> bool {
    use std::{fs::File, io, os::fd::AsFd, os::unix::fs::MetadataExt};

    let Some(journal_stream) = std::env::var_os("JOURNAL_STREAM") else {
        return false;
    };
    let Ok(stdout) = io::stdout()
        .as_fd()
        .try_clone_to_owned()
        .map(File::from)
        .and_then(|stdout| stdout.metadata())
    else {
        return false;
    };

    journal_stream.to_str() == Some(format!("{}:{}", stdout.dev(), stdout.ino()).as_str())
}

#[cfg(not(unix))]
fn stdout_is_journal() -> bool {
    false
}
