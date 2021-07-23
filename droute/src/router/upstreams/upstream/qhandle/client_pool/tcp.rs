// Copyright 2020 LEXUGE
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use super::{ClientWrapper, Result};
use async_trait::async_trait;
use std::{net::SocketAddr, time::Duration};
use tokio::net::TcpStream as TokioTcpStream;
use trust_dns_client::{client::AsyncClient, tcp::TcpClientStream};
use trust_dns_proto::iocompat::AsyncIoTokioAsStd;

/// Client instance for TCP connections
#[derive(Clone)]
pub struct Tcp {
    addr: SocketAddr,
    timeout: Duration,
}

impl Tcp {
    /// Create a new TCP client creator instance. with the given remote server address.
    pub fn new(addr: SocketAddr, timeout: Duration) -> Self {
        Self { addr, timeout }
    }
}

#[async_trait]
impl ClientWrapper<AsyncClient> for Tcp {
    async fn create(&self) -> Result<AsyncClient> {
        let (stream, sender) = TcpClientStream::<AsyncIoTokioAsStd<TokioTcpStream>>::with_timeout(
            self.addr,
            self.timeout,
        );
        let (client, bg) = AsyncClient::new(Box::new(stream), sender, None).await?;

        tokio::spawn(bg);
        Ok(client)
    }

    fn conn_type(&self) -> &'static str {
        "TCP"
    }
}
