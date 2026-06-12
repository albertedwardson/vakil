use anyhow::Result;
use async_trait::async_trait;
use vakil_l4::{TcpProxyHooks, UdpProxyHooks};
use vakil_plugin_sys::{TCPContext, UDPContext};

use crate::PluginProxyHooks;

#[async_trait]
impl TcpProxyHooks for PluginProxyHooks {
    async fn tcp_connection_init(&self, ctx: &mut TCPContext) -> Result<()> {
        for module in self.tcp_modules.iter() {
            if let Some(cb) = module.on_route.as_ref() {
                (*cb)(module.instance, &mut *ctx);
            }
            if let Some(cb) = module.on_connect.as_ref() {
                (*cb)(module.instance, &mut *ctx);
            }
        }
        Ok(())
    }

    async fn tcp_data_received(&self, ctx: &mut TCPContext) -> Result<()> {
        for module in self.tcp_modules.iter() {
            if let Some(cb) = module.on_data.as_ref() {
                (*cb)(module.instance, &mut *ctx);
            }
        }
        Ok(())
    }

    async fn tcp_connection_close(&self, ctx: &mut TCPContext) -> Result<()> {
        for module in self.tcp_modules.iter() {
            if let Some(cb) = module.on_close.as_ref() {
                (*cb)(module.instance, &mut *ctx);
            }
        }
        Ok(())
    }
}

#[async_trait]
impl UdpProxyHooks for PluginProxyHooks {
    async fn udp_session_start(&self, ctx: &mut UDPContext) -> Result<()> {
        for module in self.udp_modules.iter() {
            if let Some(cb) = module.on_session_start.as_ref() {
                (*cb)(module.instance, &mut *ctx);
            }
        }
        Ok(())
    }

    async fn udp_datagram_received(&self, ctx: &mut UDPContext) -> Result<()> {
        for module in self.udp_modules.iter() {
            if let Some(cb) = module.on_datagram.as_ref() {
                (*cb)(module.instance, &mut *ctx);
            }
        }
        Ok(())
    }

    async fn udp_session_close(&self, ctx: &mut UDPContext) -> Result<()> {
        for module in self.udp_modules.iter() {
            if let Some(cb) = module.on_session_end.as_ref() {
                (*cb)(module.instance, &mut *ctx);
            }
        }
        Ok(())
    }
}
