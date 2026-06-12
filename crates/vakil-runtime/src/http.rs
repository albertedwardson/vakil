use async_trait::async_trait;
use log::{debug, warn};
use pingora_core::ErrorType::HTTPStatus;
use pingora_core::upstreams::peer::HttpPeer;
use vakil_http::HttpProxyHooks;
use vakil_plugin_sys::{HookAction, HookOutcome, HttpContext};

use crate::PluginProxyHooks;

#[async_trait]
impl HttpProxyHooks for PluginProxyHooks {
    async fn early_request_filter(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &mut HttpContext,
    ) -> pingora_error::Result<()> {
        *ctx = session.into();
        debug!("{:?} ctx after `early_request_filter`", ctx);
        Ok(())
    }
    async fn upstream_peer(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &mut HttpContext,
    ) -> pingora_error::Result<Box<HttpPeer>> {
        debug!("{:?} ctx at the beginning of `upstream_peer`", ctx);
        for module in self.http_modules.clone().iter() {
            if let Some(route_cb) = module.on_route.as_ref() {
                let decision = (*route_cb)(module.instance, &mut *ctx)
                    .match_owned(|decision| decision, |_| HookOutcome::default());
                debug!("{:?} after module {:?} `on_route`", ctx, module);
                if ctx.clone().route().is_none() {
                    debug!("no route decision was set by {:?}", module);
                    continue;
                }
                let route = &ctx.clone().route().unwrap();
                match decision.action {
                    HookAction::Continue => {
                        continue;
                    }
                    HookAction::Replace => {
                        if route.upstream_to_set.is_none() {
                            warn!("no upstream_to_set was set by {:?}", module);
                            continue;
                        }
                        let upstream = route.clone().upstream_to_set.unwrap();
                        debug!("replacing upstream to {:?} ", upstream);
                        let path: String = route.clone().http_path.unwrap().into();
                        let new_uri = http::Uri::builder().path_and_query(path).build().unwrap();
                        debug!("uri to set: {}", new_uri);
                        session.req_header_mut().set_uri(new_uri);
                        return Ok(Box::new(HttpPeer::new(
                            (upstream.host.as_str(), upstream.port),
                            is_port_https(upstream.port),
                            upstream.host.as_str().into(),
                        )));
                    }
                    HookAction::Drop => {
                        return pingora_error::Error::e_explain(
                            HTTPStatus(503),
                            "plugin rejected request",
                        );
                    }
                }
            }
        }
        pingora_error::Error::e_explain(HTTPStatus(500), "no upstream was selected by plugins")
    }

    async fn request_filter(
        &self,
        session: &mut pingora_proxy::Session,
        ctx: &mut HttpContext,
    ) -> pingora_core::Result<bool> {
        debug!("{:?} ctx at the beginning of `request_filter`", ctx);
        debug!(
            "{:?} req header at the beginning of `request_filter`",
            session.req_header()
        );
        if ctx.route().is_some() {
            let route = ctx.route().unwrap();
            if route.http_path.is_some() {
                let mut path: String = route.http_path.unwrap().into();
                let uri = &session.req_header_mut().uri;
                if let Some(q) = uri.query() {
                    path = format!("{}{}", path, q);
                }
                let new_uri = http::Uri::builder()
                    // .authority(uri.authority().unwrap_or(&http::uri::Authority::from_static("".
                    // into())).as_str())
                    .path_and_query(path)
                    .build()
                    .unwrap();
                session.req_header_mut().set_uri(new_uri);
            }
        }

        let mut short_circuit: Option<(u16, bytes::Bytes)> = None;

        for module in self.http_modules.iter() {
            if let Some(request_cb) = module.on_request_headers.as_ref() {
                let outcome = (*request_cb)(module.instance, &mut *ctx)
                    .match_owned(|outcome| outcome, |_| HookOutcome::default());

                if ctx.response.is_some() || outcome.action as u8 == HookAction::Replace as u8 {
                    if let Some(reply_cb) = module.on_local_reply.as_ref() {
                        let _ = (*reply_cb)(module.instance, &mut *ctx)
                            .match_owned(|outcome| outcome, |_| HookOutcome::default());
                    }

                    if let Some(response) = ctx.response.as_ref() {
                        short_circuit = Some((
                            response.status.0,
                            bytes::Bytes::from(response.body.0.as_slice().to_vec()),
                        ));
                        break;
                    }
                }
            }
        }

        if let Some((status, body)) = short_circuit {
            session.respond_error_with_body(status, body).await?;
            return Ok(true);
        }

        Ok(false)
    }

    async fn request_body_filter(
        &self,
        _session: &mut pingora_proxy::Session,
        body: &mut Option<bytes::Bytes>,
        _end_of_stream: bool,
        ctx: &mut HttpContext,
    ) -> pingora_core::Result<()> {
        populate_req_body_into_ctx(body, ctx);
        for module in self.http_modules.iter() {
            if let Some(request_body_cb) = module.on_request_body.as_ref() {
                let _ = (*request_body_cb)(module.instance, &mut *ctx)
                    .match_owned(|outcome| outcome, |_| HookOutcome::default());
            }
        }

        Ok(())
    }
    async fn upstream_request_filter(
        &self,
        session: &mut pingora_proxy::Session,
        upstream_request: &mut pingora_http::RequestHeader,
        ctx: &mut HttpContext,
    ) -> pingora_core::Result<()> {
        debug!(
            "{:?} ctx at the beginning of `upstream_request_filter`",
            ctx
        );
        if ctx.route().is_some() {
            let route = ctx.route().unwrap();
            if route.http_path.is_some() {
                upstream_request
                    .insert_header("Host", route.upstream_to_set.unwrap().host.as_str())
                    .unwrap();
            }
        }

        let mut short_circuit: Option<(u16, bytes::Bytes)> = None;

        for module in self.http_modules.iter() {
            if let Some(request_cb) = module.on_request_headers.as_ref() {
                let outcome = (*request_cb)(module.instance, &mut *ctx)
                    .match_owned(|outcome| outcome, |_| HookOutcome::default());

                if ctx.response.is_some() || outcome.action as u8 == HookAction::Replace as u8 {
                    if let Some(reply_cb) = module.on_local_reply.as_ref() {
                        let _ = (*reply_cb)(module.instance, &mut *ctx)
                            .match_owned(|outcome| outcome, |_| HookOutcome::default());
                    }

                    if let Some(response) = ctx.response.as_ref() {
                        short_circuit = Some((
                            response.status.0,
                            bytes::Bytes::from(response.body.0.as_slice().to_vec()),
                        ));
                        break;
                    }
                }
            }
        }

        if let Some((status, body)) = short_circuit {
            session.respond_error_with_body(status, body).await?;
        }

        Ok(())
    }
    async fn response_filter(
        &self,
        _session: &mut pingora_proxy::Session,
        _upstream_response: &mut pingora_http::ResponseHeader,
        ctx: &mut HttpContext,
    ) -> pingora_core::Result<()> {
        for module in self.http_modules.iter() {
            if let Some(response_cb) = module.on_response_headers.as_ref() {
                let _ = (*response_cb)(module.instance, &mut *ctx)
                    .match_owned(|outcome| outcome, |_| HookOutcome::default());
            }
        }

        Ok(())
    }
    fn response_body_filter(
        &self,
        _session: &mut pingora_proxy::Session,
        body: &mut Option<bytes::Bytes>,
        end_of_stream: bool,
        ctx: &mut HttpContext,
    ) -> pingora_core::Result<Option<std::time::Duration>> {
        populate_resp_body_into_ctx(body, ctx);

        for module in self.http_modules.iter() {
            if let Some(response_body_cb) = module.on_response_body.as_ref() {
                let _ = (*response_body_cb)(module.instance, &mut *ctx)
                    .match_owned(|outcome| outcome, |_| HookOutcome::default());
            }

            if end_of_stream && let Some(trailers_cb) = module.on_trailers.as_ref() {
                let _ = (*trailers_cb)(module.instance, &mut *ctx)
                    .match_owned(|outcome| outcome, |_| HookOutcome::default());
            }
        }

        Ok(None)
    }
}

#[inline]
fn is_port_https(port: u16) -> bool {
    port == 443
}
#[inline]
fn populate_resp_body_into_ctx(body: &mut Option<bytes::Bytes>, ctx: &mut HttpContext) {
    if let Some(chunk) = body.as_ref()
        && let Some(mut resp) = ctx.response.as_mut()
    {
        resp.body.0.extend(chunk.clone());
    }
}
#[inline]
fn populate_req_body_into_ctx(body: &mut Option<bytes::Bytes>, ctx: &mut HttpContext) {
    if let Some(chunk) = body.as_ref()
        && let Some(mut req) = ctx.request.as_mut()
    {
        req.body.0.extend(chunk.clone());
    }
}
