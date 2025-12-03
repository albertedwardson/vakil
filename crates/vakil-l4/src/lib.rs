use anyhow::{Result, anyhow};
use log::error;
use rama_core::{Context, Service};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vakil_plugin_sys::{
    ConnectionInfo, FlowDirection, ID, SocketAddress, TCPContext, TransportContext,
    TransportProtocol, UDPContext,
};

use async_trait::async_trait;
use rama_tcp::{TcpStream, server::TcpListener};
use rama_udp::UdpSocket;

#[async_trait]
pub trait TcpProxyHooks: Send + Sync {
    async fn tcp_connection_init(&self, _ctx: &mut TCPContext) -> Result<()> {
        Ok(())
    }

    async fn tcp_data_received(&self, _ctx: &mut TCPContext) -> Result<()> {
        Ok(())
    }

    async fn tcp_connection_close(&self, _ctx: &mut TCPContext) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
pub trait UdpProxyHooks: Send + Sync {
    async fn udp_session_start(&self, _ctx: &mut UDPContext) -> Result<()> {
        Ok(())
    }

    async fn udp_datagram_received(&self, _ctx: &mut UDPContext) -> Result<()> {
        Ok(())
    }

    async fn udp_session_close(&self, _ctx: &mut UDPContext) -> Result<()> {
        Ok(())
    }
}

pub struct NoopProxyHooks;

#[async_trait]
impl TcpProxyHooks for NoopProxyHooks {}
#[async_trait]
impl UdpProxyHooks for NoopProxyHooks {}

pub struct TcpProxyService<H = NoopProxyHooks> {
    hooks: Arc<H>,
}

impl<H> TcpProxyService<H> {
    pub fn new(hooks: H) -> Self {
        Self {
            hooks: Arc::new(hooks),
        }
    }
}

impl<H, S> Service<S, TcpStream> for TcpProxyService<H>
where
    H: TcpProxyHooks + Send + Sync + 'static,
    S: Send + Sync + 'static,
{
    type Response = ();
    type Error = anyhow::Error;

    async fn serve(&self, _ctx: Context<S>, stream: TcpStream) -> Result<()> {
        let connection_id = ID(rand::random());

        let tcp_ctx = Arc::new(TCPContext {
            chunk: Default::default(),
            meta: TransportContext {
                connection: ConnectionInfo {
                    id: connection_id,
                    local_addr: stream.local_addr()?.into(),
                    peer_addr: Default::default(),
                    protocol: TransportProtocol::Tcp,
                },
                direction: Default::default(),
                route: Default::default(),
            },
        });

        self.hooks
            .tcp_connection_init(&mut (*tcp_ctx).clone())
            .await?;

        let upstream_addr: Option<SocketAddress> =
            tcp_ctx.meta.route.upstream_to_set.clone().into();
        let upstream_addr =
            upstream_addr.ok_or_else(|| anyhow!("plugin did not select an upstream"))?;
        let upstream: std::net::SocketAddr = upstream_addr.into();
        let upstream = TcpStream::connect(upstream).await?;

        let (mut down_r, mut down_w) = stream.into_split();
        let (mut up_r, mut up_w) = upstream.into_split();

        let hooks = self.hooks.clone();

        let mut curr_ctx = (*tcp_ctx).clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 8192];

            loop {
                match up_r.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut data = buf[..n].to_vec();
                        curr_ctx.chunk = Some((&mut data).into()).into();
                        curr_ctx.meta.direction = Some(FlowDirection::Outbound).into();
                        let hook_outcome = hooks.tcp_data_received(&mut curr_ctx).await;
                        if let Err(err) = hook_outcome {
                            error!("{} plugin returned error", err);
                            break;
                        }
                        if down_w.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }

            let _ = hooks.tcp_connection_close(&mut curr_ctx).await;
        });

        let mut buffer = vec![0u8; 8192];

        let mut curr_ctx = (*tcp_ctx).clone();
        loop {
            let n = match down_r.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };

            let mut data = buffer[..n].to_vec();

            curr_ctx.chunk = Some((&mut data).into()).into();
            curr_ctx.meta.direction = Some(FlowDirection::Inbound).into();

            self.hooks.tcp_data_received(&mut curr_ctx).await?;
            if let Some(data_to_write) = curr_ctx.chunk.as_ref()
                && up_w.write_all(data_to_write.0.as_slice()).await.is_err()
            {
                break;
            }
        }

        self.hooks
            .tcp_connection_close(&mut (*tcp_ctx).clone())
            .await?;

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// UDP service
// -----------------------------------------------------------------------------
#[derive(Clone)]
pub struct UdpProxyService<H = NoopProxyHooks> {
    hooks: Arc<H>,
}

impl<H> UdpProxyService<H> {
    pub fn new(hooks: H) -> Self {
        Self {
            hooks: Arc::new(hooks),
        }
    }
}

impl<H, S> Service<S, UdpSocket> for UdpProxyService<H>
where
    H: UdpProxyHooks + Send + Sync + 'static,
    S: Clone + Send + Sync + 'static,
{
    type Response = ();
    type Error = anyhow::Error;

    async fn serve(
        &self,
        _ctx: Context<S>,
        socket: UdpSocket,
    ) -> Result<Self::Response, Self::Error> {
        let mut buf = [0u8; 2048];

        let connection_id = ID(rand::random());
        // connection start
        let udp_ctx = Arc::new(UDPContext {
            datagram: Default::default(),
            meta: TransportContext {
                connection: ConnectionInfo {
                    id: connection_id,
                    local_addr: socket.local_addr()?.into(),
                    peer_addr: Default::default(),
                    protocol: TransportProtocol::Udp,
                },
                direction: Default::default(),
                route: Default::default(),
            },
        });
        self.hooks
            .udp_session_start(&mut (*udp_ctx).clone())
            .await?;
        let mut curr_ctx = (*udp_ctx).clone();

        loop {
            let (len, peer_addr) = socket.recv_from(&mut buf).await?;
            if len == 0 {
                break;
            }

            let mut data = buf[..len].to_vec();

            curr_ctx.meta.connection.peer_addr = Some(SocketAddress {
                host: peer_addr.ip_addr().to_string().into(),
                port: peer_addr.port(),
            })
            .into();
            curr_ctx.datagram = Some((&mut data).into()).into();

            self.hooks.udp_datagram_received(&mut curr_ctx).await?;

            let upstream_addr: Option<SocketAddress> =
                udp_ctx.meta.route.upstream_to_set.clone().into();
            let upstream_addr =
                upstream_addr.ok_or_else(|| anyhow!("plugin did not select an upstream"))?;
            let upstream: std::net::SocketAddr = upstream_addr.into();

            socket
                .send_to(&data, upstream)
                .await
                .expect("error sending data to upstream");
        }

        self.hooks.udp_session_close(&mut curr_ctx).await?;
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Launchers
// -----------------------------------------------------------------------------

pub async fn run_tcp_server<H>(listen_addr: SocketAddr, hooks: H) -> Result<()>
where
    H: TcpProxyHooks + Send + Sync + 'static,
{
    let service = TcpProxyService::new(hooks);
    let listener = TcpListener::bind(listen_addr).await.unwrap();
    listener.serve(service).await;
    Ok(())
}

pub async fn run_udp_server<H>(listen_addr: SocketAddr, hooks: H) -> Result<()>
where
    H: UdpProxyHooks + Send + Sync + 'static,
{
    let service = UdpProxyService::new(hooks);
    let socket = UdpSocket::bind(listen_addr).await.unwrap();

    service.serve(Context::default(), socket).await?;

    Ok(())
}
