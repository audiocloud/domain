use std::any::Any;
use std::env;

use clap::{Args, ValueEnum};

mod opentelemetry;
mod sentry;

#[derive(Args, Clone, Debug)]
pub struct O11yOpts {
    /// How to export tracing data
    #[clap(long, env, default_value = "off")]
    tracing: TracingMode,

    /// The OTLP collector or Grafana agent OTLP endpoint
    ///
    /// GRPC and HTTP endpoints should be supported.
    #[clap(long, env, default_value = "grpc://localhost:4317")]
    otlp_endpoint: String,

    /// Timeout to write metrics and traces to OTLP, in milliseconds
    #[clap(long, env, default_value = "5000")]
    otlp_timeout_ms: u64,

    /// Sentry DSN, if tracing is set to 'sentry'
    ///
    /// You can get this from the Sentry project settings page, as of time of this writing it is an URL pointing to ingest.sentry.io
    #[clap(long, env)]
    sentry_dsn: Option<String>,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum TracingMode {
    /// Do not export tracing data
    Off,

    /// OTLP compatible exporter (GRPC)
    OpenTracing,

    /// Sentry compatible exporter
    Sentry,
}

pub fn init(opts: &O11yOpts) -> anyhow::Result<Box<dyn Any>> {
    set_rust_log_defaults();

    match opts.tracing {
        TracingMode::Off => {
            tracing_subscriber::fmt::init();
            Ok(Box::new(()))
        }
        TracingMode::OpenTracing => {
            opentelemetry::setup_opentelemetry(opts)?;
            Ok(Box::new(()))
        }
        TracingMode::Sentry => Ok(sentry::setup_sentry(opts)?),
    }
}

fn set_rust_log_defaults() {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG",
                     "info,audiocloud_domain_server=debug,audiocloud_api=debug,actix_server=warn,rdkafka=debug");
    }
}
