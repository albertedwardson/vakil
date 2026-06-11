use anyhow::{Result, anyhow};
use async_trait::async_trait;
use log::{debug, error};
use rama::error::BoxError;
use rama_core::Service;
use rama_tcp::{TcpStream, server::TcpListener};
use rama_udp::UdpSocket;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vakil_plugin_sys::{
    ConnectionInfo, FlowDirection, ID, SocketAddress, TCPContext, TransportContext,
    TransportProtocol, UDPContext,
};

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

#[derive(Debug)]
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

impl<H> Service<TcpStream> for TcpProxyService<H>
where
    H: TcpProxyHooks + Send + Sync + 'static,
{
    type Output = ();
    type Error = anyhow::Error;

    async fn serve(&self, stream: TcpStream) -> Result<(), Self::Error> {
        debug!("tcp started serving");
        let connection_id = ID(rand::random());

        let tcp_ctx = Arc::new(TCPContext {
            chunk: Default::default(),
            meta: TransportContext {
                connection: ConnectionInfo {
                    id: connection_id,
                    local_addr: stream.stream.local_addr()?.into(),
                    peer_addr: Default::default(),
                    protocol: TransportProtocol::Tcp,
                },
                direction: Default::default(),
                route: Default::default(),
            },
        });
        debug!("{:?}", tcp_ctx);
        self.hooks
            .tcp_connection_init(&mut (*tcp_ctx).clone())
            .await?;
        debug!("{:?} after tcp_connection_init", tcp_ctx);
        let upstream_addr: Option<SocketAddress> =
            tcp_ctx.meta.route.upstream_to_set.clone().into();
        let upstream_addr =
            upstream_addr.ok_or_else(|| anyhow!("plugin did not select an upstream"))?;
        let upstream: std::net::SocketAddr = upstream_addr.into();
        let upstream = rama_tcp::client::default_tcp_connect(&stream.extensions, upstream.into())
            .await?
            .0;

        let (mut down_r, mut down_w) = stream.stream.into_split();
        let (mut up_r, mut up_w) = upstream.stream.into_split();

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

impl<H> Service<UdpSocket> for UdpProxyService<H>
where
    H: UdpProxyHooks + Send + Sync + 'static,
{
    type Output = ();
    type Error = anyhow::Error;

    async fn serve(&self, socket: UdpSocket) -> Result<(), Self::Error> {
        debug!("udp started serving");
        let mut buf = [0u8; 2048];

        let connection_id = ID(rand::random());
        // connection start
        let udp_ctx = Arc::new(UDPContext {
            datagram: Default::default(),
            meta: TransportContext {
                connection: ConnectionInfo {
                    id: connection_id,
                    local_addr: socket.local_addr().unwrap().into(),
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
                host: peer_addr.ip().to_string().into(),
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

pub async fn run_tcp_server<H>(listen_addr: SocketAddr, hooks: H) -> Result<(), BoxError>
where
    H: TcpProxyHooks + Send + Sync + 'static,
{
    debug!("entered `run_tcp_server`");
    let service = TcpProxyService::new(hooks);
    debug!(
        "`run_tcp_server`: builed service, binding to {:?}",
        listen_addr
    );
    let listener = TcpListener::bind_address(listen_addr)
        .await
        .expect("bind TCP listener");
    debug!("`run_tcp_server`: binded to socket, starting serving");
    listener.serve(service).await;
    debug!("`run_tcp_server`: serve completed");
    Ok(())
}

pub async fn run_udp_server<H>(listen_addr: SocketAddr, hooks: H) -> Result<()>
where
    H: UdpProxyHooks + Send + Sync + 'static,
{
    debug!("entered `run_udp_server`");
    let service = UdpProxyService::new(hooks);
    debug!(
        "`run_udp_server`: builed service, binding to {:?}",
        listen_addr
    );
    let socket = UdpSocket::bind(listen_addr).await.unwrap();

    debug!("`run_udp_server`: binded to socket, starting serving");
    service.serve(socket).await?;

    debug!("`run_udp_server`: serve completed");
    Ok(())
}
