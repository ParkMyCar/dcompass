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

use super::{ConnInitiator, QHandle, QHandleError, Result};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use domain::base::Message;
use once_cell::sync::Lazy;
use reqwest::{Client, Proxy, Url};
use rustls::{ClientConfig, KeyLogFile, ProtocolVersion, RootCertStore};
use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

static NO_SNI_CLIENT_CFG: Lazy<ClientConfig> = Lazy::new(|| create_client_config(&false));
static CLIENT_CFG: Lazy<ClientConfig> = Lazy::new(|| create_client_config(&true));

const ALPN_H2: &[u8] = b"h2";

fn create_client_config(sni: &bool) -> ClientConfig {
    let mut root_store = RootCertStore::empty();
    root_store.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    let versions = vec![ProtocolVersion::TLSv1_3];

    let mut client_config = ClientConfig::new();
    client_config.root_store = root_store;
    client_config.versions = versions;
    client_config.alpn_protocols.push(ALPN_H2.to_vec());
    client_config.key_log = Arc::new(KeyLogFile::new());
    client_config.enable_sni = *sni; // Disable SNI on need.

    client_config
}

/// Client instance for UDP connections
#[derive(Clone)]
pub struct Https {
    addr: IpAddr,
    proxy: Option<Proxy>,
    sni: bool,
    uri: Url,
}

static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

impl Https {
    /// Create a new HTTPS client creator instance. with the given remote server address.
    pub async fn new(uri: String, addr: IpAddr, proxy: Option<String>, sni: bool) -> Result<Self> {
        let uri = Url::from_str(&uri).map_err(|_| QHandleError::InvalidUri(uri))?;
        let _ = uri
            .domain()
            .ok_or_else(|| QHandleError::InvalidDomain(uri.clone()))?;

        Ok(Self {
            addr,
            proxy: if let Some(proxy) = proxy {
                Some(Proxy::all(proxy)?)
            } else {
                None
            },
            sni,
            uri,
        })
    }
}

#[async_trait]
impl ConnInitiator for Https {
    type Connection = PostClient;

    async fn create(&self) -> std::io::Result<Self::Connection> {
        // We cannot reuse the client because if the network changes, client pool inside each client remains the same, and cloning them inevitably leads to no reconnection but using stale connections.
        // We cannot store ClientBuilder because it is not Clone.

        // This has already been checked and it is safe to unwrap
        let domain = self.uri.domain().unwrap();
        let client = Client::builder()
            // The port in socket addr doesn't take effect here per documentation
            .resolve(domain, SocketAddr::new(self.addr, 0))
            .use_preconfigured_tls(if self.sni {
                CLIENT_CFG.clone()
            } else {
                NO_SNI_CLIENT_CFG.clone()
            })
            .https_only(true)
            .user_agent(APP_USER_AGENT)
            .connect_timeout(Duration::from_secs(3))
            .pool_max_idle_per_host(32);

        let client = if let Some(proxy) = self.proxy.clone() {
            client.proxy(proxy)
        } else {
            client
        };

        Ok(PostClient(
            client.build().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "TLS backend failed to initialize",
                )
            })?,
            self.uri.clone(),
        ))
    }

    fn conn_type(&self) -> &'static str {
        "HTTPS"
    }
}

pub struct PostClient(Client, Url);

#[async_trait]
impl QHandle for PostClient {
    async fn query(&self, msg: &Message<Bytes>) -> Result<Message<Bytes>> {
        // Per RFC, the message ID should be set to 0 to better facilitate HTTPS caching.
        let mut msg = Message::from_octets(BytesMut::from(msg.as_slice()))?;
        msg.header_mut().set_id(0);

        let body: reqwest::Body = msg.into_octets().freeze().into();
        let res = self
            .0
            .post(self.1.clone())
            .header("content-type", "application/dns-message")
            .body(body)
            .send()
            .await?;

        if res.status().is_success() {
            let res = res.bytes().await?;
            let answer = Message::from_octets(res)?;
            Ok(answer)
        } else {
            Err(QHandleError::FailedHttp(res.status()))
        }
    }
}
