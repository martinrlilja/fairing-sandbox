use anyhow::Result;
use std::{future::Future, net::SocketAddr, pin::Pin, str::FromStr, sync::Arc, time::Duration};
use tokio::net::{TcpListener, UdpSocket};
use trust_dns_client::rr::LowerName;
use trust_dns_proto::{
    op::ResponseCode,
    rr::{rdata, Name, RData, RecordSet, RecordType},
};
use trust_dns_server::{
    authority::{
        AuthLookup, AuthorityObject, Catalog, LookupError, LookupObject, LookupOptions,
        LookupRecords, MessageRequest, UpdateResult, ZoneType,
    },
    server::RequestInfo,
};

use fairing_acme::ES256PublicKey;
use fairing_core2::services::DomainService;

pub async fn serve(
    domains: DomainService,
    zone: String,
    udp_addr: Vec<SocketAddr>,
    tcp_addr: Vec<SocketAddr>,
) -> Result<()> {
    let zone = LowerName::from_str(&zone)?;

    let authority = Authority {
        origin: zone.base_name(),
        domains,
    };

    let mut catalog = Catalog::new();
    catalog.upsert(zone, Box::new(authority));

    let mut server = trust_dns_server::ServerFuture::new(catalog);

    for udp_addr in udp_addr {
        let udp_socket = UdpSocket::bind(udp_addr).await?;
        server.register_socket(udp_socket);

        tracing::info!("acme dns listening on udp {udp_addr}");
    }

    for tcp_addr in tcp_addr {
        let tcp_listener = TcpListener::bind(tcp_addr).await?;
        server.register_listener(tcp_listener, Duration::from_secs(30));

        tracing::info!("acme dns listening on tcp {tcp_addr}");
    }

    server.block_until_done().await?;

    Ok(())
}

struct Authority {
    origin: LowerName,
    domains: DomainService,
}

impl AuthorityObject for Authority {
    fn box_clone(&self) -> Box<dyn AuthorityObject> {
        Box::new(Authority {
            origin: self.origin.clone(),
            domains: self.domains,
        })
    }

    fn zone_type(&self) -> ZoneType {
        ZoneType::Primary
    }

    fn is_axfr_allowed(&self) -> bool {
        false
    }

    fn origin(&self) -> &LowerName {
        &self.origin
    }

    fn update<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _update: &'life1 MessageRequest,
    ) -> Pin<Box<dyn Future<Output = UpdateResult<bool>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { UpdateResult::Err(ResponseCode::NotImp) })
    }

    fn lookup<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _name: &'life1 LowerName,
        _rtype: RecordType,
        _lookup_options: LookupOptions,
    ) -> Pin<
        Box<dyn Future<Output = Result<Box<dyn LookupObject>, LookupError>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Err(LookupError::ResponseCode(ResponseCode::NotImp)) })
    }

    fn search<'life0, 'life1, 'async_trait>(
        &'life0 self,
        request_info: RequestInfo<'life1>,
        lookup_options: LookupOptions,
    ) -> Pin<
        Box<dyn Future<Output = Result<Box<dyn LookupObject>, LookupError>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            match request_info.query.query_type() {
                RecordType::ANY | RecordType::TXT => {
                    let name: Name = request_info.query.name().into();
                    let acme_label = name
                        .iter()
                        .next()
                        .and_then(|acme_label| String::from_utf8(acme_label.to_owned()).ok());

                    let acme_label = match acme_label {
                        Some(acme_label) => acme_label,
                        None => return Ok(Box::new(AuthLookup::Empty) as Box<dyn LookupObject>),
                    };

                    let challenges = self
                        .domains
                        .get_acme_dns_01_challenges(&acme_label)
                        .await
                        .map_err(|err| {
                            tracing::error!("error looking up acme challenge: {:?}", err);
                            LookupError::ResponseCode(ResponseCode::ServFail)
                        })?;

                    if challenges.is_empty() {
                        return Ok(Box::new(AuthLookup::Empty) as Box<dyn LookupObject>);
                    }

                    /*
                    let key_authorization = self
                        .public_key
                        .dns_key_authorization(&challenge.dns_01_token);
                    */

                    let mut records =
                        RecordSet::with_ttl(request_info.query.name().into(), RecordType::TXT, 60);
                    records.add_rdata(RData::TXT(rdata::TXT::new(challenges)));

                    let answers = LookupRecords::Records {
                        lookup_options,
                        records: Arc::new(records),
                    };

                    Ok(Box::new(AuthLookup::Records {
                        answers,
                        additionals: None,
                    }) as Box<dyn LookupObject>)
                }
                _ => Ok(Box::new(AuthLookup::Empty) as Box<dyn LookupObject>),
            }
        })
    }

    fn get_nsec_records<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _name: &'life1 LowerName,
        _lookup_options: LookupOptions,
    ) -> Pin<
        Box<dyn Future<Output = Result<Box<dyn LookupObject>, LookupError>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Err(LookupError::ResponseCode(ResponseCode::NotImp)) })
    }
}
