/// Builtin logging capabilities available within `wasmcloud:builtin:logging` namespace
pub mod logging;
/// Builtin random number generation capabilities available within `wasmcloud:builtin:numbergen` namespace
pub mod numbergen;

pub use logging::*;
pub use numbergen::*;

use core::fmt::Debug;

use anyhow::{bail, Context};
use tracing::{instrument, trace_span};
use wasmbus_rpc::common::{deserialize, serialize};
use wasmcloud_interface_logging::LogEntry;
use wasmcloud_interface_numbergen::RangeLimit;

/// Capability handler
pub trait Handler {
    /// Error returned by [`Handler::handle`] operations
    type Error: ToString + Debug;

    /// Handles a raw capability provider invocation.
    ///
    /// # Errors
    ///
    /// Returns an [`anyhow::Error`] in case an error is non-recoverable, for example if an invalid
    /// payload is passed to a builtin provider, which will cause an exception in the guest.
    /// Innermost result represents the underlying operation result, which will be passed to the
    /// guest as an application-layer error.
    fn handle(
        &self,
        binding: String,
        namespace: String,
        operation: String,
        payload: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, Self::Error>>;
}

/// A [Handler], which handles all builtin capability invocations using [Logging], [Numbergen] and
/// offloads all external capabilities to an arbitrary [Handler]
pub struct BuiltinHandler<L, N, H> {
    /// Logging capability provider, using which all known `wasmcloud:builtin:logging` operations will be handled
    pub logging: L,

    /// Random number generator capability provider, using which all known `wasmcloud:builtin:numbergen` operations will be handled
    pub numbergen: N,

    /// External capability provider, using which all non-builtin calls will be handled
    pub external: H,
}

impl Handler for () {
    type Error = &'static str;

    fn handle(
        &self,
        _: String,
        _: String,
        _: String,
        _: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, Self::Error>> {
        Ok(Err("not supported"))
    }
}

impl<T, E, F> Handler for F
where
    T: Into<Vec<u8>>,
    E: ToString + Debug,
    F: Fn(String, String, String, Vec<u8>) -> anyhow::Result<Result<T, E>>,
{
    type Error = E;

    fn handle(
        &self,
        binding: String,
        namespace: String,
        operation: String,
        payload: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, Self::Error>> {
        match self(binding, namespace, operation, payload) {
            Ok(Ok(res)) => Ok(Ok(res.into())),
            Ok(Err(err)) => Ok(Err(err)),
            Err(err) => Err(err),
        }
    }
}

impl<L, N, H> Handler for BuiltinHandler<L, N, H>
where
    L: Logging,
    N: Numbergen,
    H: Handler,
{
    type Error = String;

    #[instrument(skip(self))]
    fn handle(
        &self,
        binding: String,
        namespace: String,
        operation: String,
        payload: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, Self::Error>> {
        match (binding.as_str(), namespace.as_str(), operation.as_str()) {
            (_, "wasmcloud:builtin:logging", "Logging.WriteLog") => {
                let LogEntry { level, text } =
                    deserialize(&payload).context("failed to deserialize log entry")?;
                let res = match level.as_str() {
                    "debug" => trace_span!("Logging::debug").in_scope(|| self.logging.debug(text)),
                    "info" => trace_span!("Logging::info").in_scope(|| self.logging.info(text)),
                    "warn" => trace_span!("Logging::warn").in_scope(|| self.logging.warn(text)),
                    "error" => trace_span!("Logging::error").in_scope(|| self.logging.error(text)),
                    _ => {
                        bail!("log level `{level}` is not supported")
                    }
                };
                match res {
                    Ok(()) => Ok(Ok(vec![])),
                    Err(err) => Ok(Err(err.to_string())),
                }
            }
            (_, "wasmcloud:builtin:numbergen", "NumberGen.GenerateGuid") => {
                match trace_span!("Numbergen::generate_guid")
                    .in_scope(|| self.numbergen.generate_guid())
                {
                    Ok(guid) => serialize(&guid.to_string())
                        .context("failed to serialize UUID")
                        .map(Ok),
                    Err(err) => Ok(Err(err.to_string())),
                }
            }
            (_, "wasmcloud:builtin:numbergen", "NumberGen.RandomInRange") => {
                let RangeLimit { min, max } =
                    deserialize(&payload).context("failed to deserialize range limit")?;
                match trace_span!("Numbergen::random_in_range")
                    .in_scope(|| self.numbergen.random_in_range(min, max))
                {
                    Ok(v) => serialize(&v).context("failed to serialize number").map(Ok),
                    Err(err) => Ok(Err(err.to_string())),
                }
            }
            (_, "wasmcloud:builtin:numbergen", "NumberGen.Random32") => {
                match trace_span!("Numbergen::random_32").in_scope(|| self.numbergen.random_32()) {
                    Ok(v) => serialize(&v).context("failed to serialize number").map(Ok),
                    Err(err) => Ok(Err(err.to_string())),
                }
            }
            _ => match trace_span!("Handler::handle")
                .in_scope(|| self.external.handle(binding, namespace, operation, payload))
            {
                Ok(Ok(res)) => Ok(Ok(res)),
                Ok(Err(err)) => Ok(Err(err.to_string())),
                Err(err) => Err(err),
            },
        }
    }
}