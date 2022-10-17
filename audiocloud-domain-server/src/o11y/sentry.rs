use std::any::Any;

use anyhow::anyhow;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::o11y::O11yOpts;

pub fn setup_sentry(opts: &O11yOpts) -> anyhow::Result<Box<dyn Any>> {
    let dsn = opts.sentry_dsn.as_ref().ok_or_else(|| anyhow!("Sentry DSN not set"))?;

    let guard = sentry::init((dsn.as_str(),
                              sentry::ClientOptions { // Set this a to lower value in production
                                                      traces_sample_rate: 1.0,
                                                      ..sentry::ClientOptions::default() }));

    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer().with_filter(EnvFilter::from_default_env()))
                                  .with(sentry_tracing::layer())
                                  .init();

    Ok(Box::new(guard))
}
