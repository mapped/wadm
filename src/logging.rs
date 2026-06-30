use std::io::IsTerminal;

use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::{
    trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
    Resource,
};
use tracing::{Event as TracingEvent, Subscriber};
use tracing_subscriber::fmt::{
    format::{Format, Full, Json, JsonFields, Writer},
    time::SystemTime,
    FmtContext, FormatEvent, FormatFields,
};
use tracing_subscriber::{layer::SubscriberExt, registry::LookupSpan, EnvFilter, Layer};

const TRACING_PATH: &str = "/v1/traces";

/// A struct that allows us to dynamically choose JSON formatting without using dynamic dispatch.
/// This is just so we avoid any sort of possible slow down in logging code
enum JsonOrNot {
    Not(Format<Full, SystemTime>),
    Json(Format<Json, SystemTime>),
}

impl<S, N> FormatEvent<S, N> for JsonOrNot
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        writer: Writer<'_>,
        event: &TracingEvent<'_>,
    ) -> std::fmt::Result
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        match self {
            JsonOrNot::Not(f) => f.format_event(ctx, writer, event),
            JsonOrNot::Json(f) => f.format_event(ctx, writer, event),
        }
    }
}

pub fn configure_tracing(
    structured_logging: bool,
    tracing_enabled: bool,
    tracing_endpoint: Option<String>,
) {
    let env_filter_layer = get_env_filter();
    let log_layer = get_log_layer(structured_logging);
    let subscriber = tracing_subscriber::Registry::default()
        .with(env_filter_layer)
        .with(log_layer);
    if !tracing_enabled && tracing_endpoint.is_none() {
        if let Err(e) = tracing::subscriber::set_global_default(subscriber) {
            eprintln!("Logger/tracer was already initted, continuing: {}", e);
        }
        return;
    }

    let mut tracing_endpoint =
        tracing_endpoint.unwrap_or_else(|| format!("http://localhost:4318{}", TRACING_PATH));
    if !tracing_endpoint.ends_with(TRACING_PATH) {
        tracing_endpoint.push_str(TRACING_PATH);
    }

    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(tracing_endpoint)
        .with_protocol(Protocol::HttpBinary)
        .build();

    let res = match exporter {
        Ok(exporter) => {
            let provider = SdkTracerProvider::builder()
                .with_sampler(Sampler::AlwaysOn)
                .with_id_generator(RandomIdGenerator::default())
                .with_max_events_per_span(64)
                .with_max_attributes_per_span(16)
                .with_resource(Resource::builder().with_service_name("wadm").build())
                .with_batch_exporter(exporter)
                .build();
            let tracer = opentelemetry::trace::TracerProvider::tracer(&provider, "wadm");
            opentelemetry::global::set_tracer_provider(provider);
            tracing::subscriber::set_global_default(
                subscriber.with(tracing_opentelemetry::layer().with_tracer(tracer)),
            )
        }
        Err(e) => {
            eprintln!(
                "Unable to configure OTEL tracing, defaulting to logging only: {:?}",
                e
            );
            tracing::subscriber::set_global_default(subscriber)
        }
    };
    if let Err(e) = res {
        eprintln!("Logger/tracer was already initted, continuing: {}", e);
    }
}

fn get_log_layer<S>(structured_logging: bool) -> Box<dyn Layer<S> + Send + Sync + 'static>
where
    S: for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    S: tracing::Subscriber,
{
    let log_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal());
    if structured_logging {
        Box::new(
            log_layer
                .event_format(JsonOrNot::Json(Format::default().json()))
                .fmt_fields(JsonFields::default()),
        )
    } else {
        Box::new(log_layer.event_format(JsonOrNot::Not(Format::default())))
    }
}

fn get_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|e| {
        eprintln!("RUST_LOG was not set or the given directive was invalid: {e:?}\nDefaulting logger to `info` level");
        EnvFilter::default().add_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
    })
}
