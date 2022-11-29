//! gRPC service implementations for `ingester`.

mod query;
mod rpc_write;

use std::{fmt::Debug, sync::Arc};

use arrow_flight::flight_service_server::FlightServiceServer;
use generated_types::influxdata::iox::{
    catalog::v1::catalog_service_server::CatalogServiceServer,
    ingester::v1::write_service_server::WriteServiceServer,
};
use iox_catalog::interface::Catalog;
use service_grpc_catalog::CatalogService;

use crate::{
    dml_sink::DmlSink,
    init::IngesterRpcInterface,
    query::{response::QueryResponse, QueryExec},
};

use self::rpc_write::RpcWrite;

/// This type is responsible for injecting internal dependencies that SHOULD NOT
/// leak outside of the ingester crate into public gRPC handlers.
///
/// Configuration and external dependencies SHOULD be injected through the
/// respective gRPC handler constructor method.
#[derive(Debug)]
pub(crate) struct GrpcDelegate<D, Q> {
    dml_sink: Arc<D>,
    query_exec: Arc<Q>,
}

impl<D, Q> GrpcDelegate<D, Q>
where
    D: DmlSink + 'static,
    Q: QueryExec<Response = QueryResponse> + 'static,
{
    /// Initialise a new [`GrpcDelegate`].
    pub(crate) fn new(dml_sink: Arc<D>, query_exec: Arc<Q>) -> Self {
        Self {
            dml_sink,
            query_exec,
        }
    }
}

/// Implement the type-erasure trait to hide internal types from crate-external
/// callers.
impl<D, Q> IngesterRpcInterface for GrpcDelegate<D, Q>
where
    D: DmlSink + 'static,
    Q: QueryExec<Response = QueryResponse> + 'static,
{
    type CatalogHandler = CatalogService;
    type WriteHandler = RpcWrite<Arc<D>>;
    type FlightHandler = query::FlightService<Arc<Q>>;

    /// Acquire a [`CatalogService`] gRPC service implementation.
    ///
    /// [`CatalogService`]: generated_types::influxdata::iox::catalog::v1::catalog_service_server::CatalogService.
    fn catalog_service(
        &self,
        catalog: Arc<dyn Catalog>,
    ) -> CatalogServiceServer<Self::CatalogHandler> {
        CatalogServiceServer::new(CatalogService::new(catalog))
    }

    /// Return a [`WriteService`] gRPC implementation.
    ///
    /// [`WriteService`]: generated_types::influxdata::iox::catalog::v1::write_service_server::WriteService.
    fn write_service(&self) -> WriteServiceServer<Self::WriteHandler> {
        WriteServiceServer::new(RpcWrite::new(Arc::clone(&self.dml_sink)))
    }

    /// Return an Arrow [`FlightService`] gRPC implementation.
    ///
    /// [`FlightService`]: arrow_flight::flight_service_server::FlightService
    fn query_service(
        &self,
        max_simultaneous_requests: usize,
        metrics: &metric::Registry,
    ) -> FlightServiceServer<Self::FlightHandler> {
        FlightServiceServer::new(query::FlightService::new(
            Arc::clone(&self.query_exec),
            max_simultaneous_requests,
            metrics,
        ))
    }
}