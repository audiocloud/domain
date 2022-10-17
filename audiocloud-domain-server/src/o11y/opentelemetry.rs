use std::time::Duration;

use opentelemetry::sdk::{trace, Resource};
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

use crate::o11y::O11yOpts;

pub fn setup_opentelemetry(opts: &O11yOpts) -> anyhow::Result<()> {
    let otlp_exporter = || {
        opentelemetry_otlp::new_exporter().tonic()
                                          .with_endpoint(&opts.otlp_endpoint)
                                          .with_timeout(Duration::from_millis(opts.otlp_timeout_ms))
    };

    let trace_config = || {
        trace::config().with_resource(Resource::new(vec![KeyValue::new("service.name", "domain"),
                                                         KeyValue::new("service.namespace", "audiocloud_io"),
                                                         KeyValue::new("service.instance.name", "distopik_hq"), // TODO: we have to configure this somehow
                                                         KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),]))
    };

    let tracer = opentelemetry_otlp::new_pipeline().tracing()
                                                   .with_exporter(otlp_exporter())
                                                   .with_trace_config(trace_config())
                                                   .install_batch(opentelemetry::runtime::Tokio)?;

    Registry::default().with(tracing_subscriber::fmt::layer().with_filter(EnvFilter::from_default_env()))
                       .with(tracing_opentelemetry::layer().with_tracer(tracer))
                       .init();

    Ok(())
}
