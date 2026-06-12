//! Pingora proxy wiring for injectable hook callbacks.
//! Actual logic implemented in hooks at ['vakil_runtime::http']

use async_trait::async_trait;
use bytes::Bytes;
use log::{debug, info, warn};
use pingora_core::prelude::*;
use pingora_core::server::Server;
use pingora_core::upstreams::peer::HttpPeer;
use pingora_error::{Error, ErrorType::HTTPStatus};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{ProxyHttp, Session, http_proxy_service};
use std::time::Duration;
use vakil_plugin_sys::HttpContext;

#[async_trait]
pub trait HttpProxyHooks: Send + Sync {
    async fn early_request_filter(
        &self,
        _session: &mut Session,
        _ctx: &mut HttpContext,
    ) -> Result<()> {
        Ok(())
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        _ctx: &mut HttpContext,
    ) -> Result<Box<HttpPeer>> {
        Error::e_explain(HTTPStatus(500), "no upstream selected")
    }

    async fn request_filter(&self, _session: &mut Session, _ctx: &mut HttpContext) -> Result<bool> {
        Ok(false)
    }

    async fn request_body_filter(
        &self,
        _session: &mut Session,
        _body: &mut Option<Bytes>,
        _end_of_stream: bool,
        _ctx: &mut HttpContext,
    ) -> Result<()> {
        Ok(())
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        _upstream_request: &mut RequestHeader,
        _ctx: &mut HttpContext,
    ) -> Result<()> {
        Ok(())
    }

    async fn response_filter(
        &self,
        _session: &mut Session,
        _upstream_response: &mut ResponseHeader,
        _ctx: &mut HttpContext,
    ) -> Result<()> {
        Ok(())
    }

    fn response_body_filter(
        &self,
        _session: &mut Session,
        _body: &mut Option<Bytes>,
        _end_of_stream: bool,
        _ctx: &mut HttpContext,
    ) -> Result<Option<Duration>> {
        Ok(None)
    }

    async fn logging(
        &self,
        session: &mut Session,
        error: Option<&pingora_core::Error>,
        ctx: &mut HttpContext,
    ) {
        if let Some(error) = error {
            warn!(
                "curr request of {} completed with error: {}",
                session.client_addr().unwrap(),
                error
            );
        }
        debug!("final request ctx: {:?}", ctx);
    }
}

#[derive(Clone, Debug, Default)]
/// Default no-op hook implementation.
pub struct NoopProxyHooks;

#[async_trait]
impl HttpProxyHooks for NoopProxyHooks {}

#[derive(Clone, Debug)]
/// Network settings used when attaching the HTTP proxy service.
pub struct ProxySettings {
    /// Address where Pingora accepts downstream connections.
    pub listen_addr: String,
}

impl ProxySettings {
    /// Create proxy settings from listen address and default upstream target.
    pub fn new(listen_addr: impl Into<String>) -> Self {
        Self {
            listen_addr: listen_addr.into(),
        }
    }
}

#[derive(Clone, Debug)]
/// Pingora HTTP proxy implementation parameterized by a hook provider.
pub struct VakilHttpProxy<H = NoopProxyHooks> {
    pub settings: ProxySettings,
    pub hooks: H,
}

impl VakilHttpProxy<NoopProxyHooks> {
    /// Construct a proxy with no-op hooks.
    pub fn new(settings: ProxySettings) -> Self {
        Self::with_hooks(settings, NoopProxyHooks)
    }
}

impl<H> VakilHttpProxy<H> {
    /// Construct a proxy with custom hooks.
    pub fn with_hooks(settings: ProxySettings, hooks: H) -> Self {
        Self { settings, hooks }
    }
}

#[async_trait]
impl<H> ProxyHttp for VakilHttpProxy<H>
where
    H: HttpProxyHooks + 'static,
{
    type CTX = HttpContext;

    fn new_ctx(&self) -> Self::CTX {
        HttpContext::default()
    }

    async fn early_request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<()> {
        self.hooks.early_request_filter(session, ctx).await
    }

    async fn upstream_peer(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        self.hooks.upstream_peer(session, ctx).await
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        self.hooks.request_filter(session, ctx).await
    }

    async fn request_body_filter(
        &self,
        session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        self.hooks
            .request_body_filter(session, body, end_of_stream, ctx)
            .await
    }

    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        self.hooks
            .upstream_request_filter(session, upstream_request, ctx)
            .await
    }

    async fn response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        self.hooks
            .response_filter(session, upstream_response, ctx)
            .await
    }

    fn response_body_filter(
        &self,
        session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<Option<Duration>> {
        self.hooks
            .response_body_filter(session, body, end_of_stream, ctx)
    }

    async fn logging(
        &self,
        session: &mut Session,
        error: Option<&pingora_core::Error>,
        ctx: &mut Self::CTX,
    ) {
        self.hooks.logging(session, error, ctx).await;
    }
}

/// Attach a configured HTTP proxy service to an existing Pingora server.
pub fn attach_proxy_service<H>(server: &mut Server, settings: ProxySettings, hooks: H)
where
    H: HttpProxyHooks + 'static,
{
    info!("attaching HTTP proxy service on {}", settings.listen_addr);
    let proxy = VakilHttpProxy::with_hooks(settings.clone(), hooks);
    let mut service = http_proxy_service(&server.configuration, proxy);
    service.add_tcp(settings.listen_addr.as_str());
    server.add_service(service);
}
