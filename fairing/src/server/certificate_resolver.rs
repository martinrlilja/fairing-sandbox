use anyhow::{anyhow, Result};
use fairing_core::backends::Database;
use futures::Stream;
use rustls::server::Acceptor;
use std::{collections::HashMap, io, sync::Arc};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::RwLock,
};
use tokio_rustls::{server::TlsStream, TlsAcceptor};

#[derive(Clone)]
pub struct CertificateResolver {
    database: Database,
    certificates: Arc<RwLock<HashMap<String, Arc<TlsAcceptor>>>>,
}

impl CertificateResolver {
    pub fn new(database: Database) -> CertificateResolver {
        CertificateResolver {
            database,
            certificates: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

pub fn accept(
    tcp_listener: TcpListener,
    certificate_resolver: CertificateResolver,
) -> impl Stream<Item = Result<TlsStream<TcpStream>, io::Error>> {
    let (sender, receiver) = tokio::sync::mpsc::channel(1024);

    tokio::task::spawn(accept_loop(tcp_listener, certificate_resolver, sender));

    tokio_stream::wrappers::ReceiverStream::new(receiver)
}

async fn accept_loop(
    tcp_listener: TcpListener,
    certificate_resolver: CertificateResolver,
    sender: tokio::sync::mpsc::Sender<Result<TlsStream<TcpStream>, io::Error>>,
) {
    loop {
        let tcp_stream = match tcp_listener.accept().await {
            Ok((tcp_stream, _)) => tcp_stream,
            Err(err) => {
                tracing::error!("failed to accept tcp stream: {}", err);
                break;
            }
        };

        let sender = sender.clone();
        let certificate_resolver = certificate_resolver.clone();
        tokio::task::spawn(async move {
            let tls_stream = accept_socket(tcp_stream, certificate_resolver).await;
            match tls_stream {
                Ok(tls_stream) => {
                    let send_result = sender.send(Ok::<_, std::io::Error>(tls_stream)).await;
                    if let Err(_) = send_result {
                        tracing::error!("dropping tls stream, too many in queue");
                    }
                }
                Err(err) => {
                    tracing::error!("error accpting tls connection: {}", err);
                }
            }
        });
    }
}

async fn accept_socket(
    tcp_stream: TcpStream,
    certificate_resolver: CertificateResolver,
) -> Result<TlsStream<TcpStream>> {
    let mut client_hello_buf = vec![0u8; 2048];
    let mut acceptor = Acceptor::new()?;

    let peeked = tcp_stream.peek(&mut client_hello_buf).await?;
    acceptor.read_tls(&mut &client_hello_buf[..peeked])?;

    let accepted = acceptor
        .accept()?
        .ok_or_else(|| anyhow!("expected a client hello"))?;

    let client_hello = accepted.client_hello();

    let sni = client_hello
        .server_name()
        .ok_or_else(|| anyhow!("client did not supply sni"))?;

    {
        let certificates = certificate_resolver.certificates.read().await;
        let tls_acceptor = certificates.get(sni);

        if let Some(tls_acceptor) = tls_acceptor {
            let tls_stream = tls_acceptor.accept(tcp_stream).await?;
            return Ok(tls_stream);
        }
    }

    let certificate = certificate_resolver
        .database
        .get_certificate(sni)
        .await?
        .ok_or_else(|| anyhow!("no certificate found"))?;

    /*
    let hosts = vec![sni.into()];

    let cert = rcgen::generate_simple_self_signed(hosts)?;

    let public = rustls::Certificate(cert.serialize_der()?);
    let private = rustls::PrivateKey(cert.serialize_private_key_der());
    */

    let public_key_chain = certificate
        .public_key_chain
        .into_iter()
        .map(|cert| rustls::Certificate(cert))
        .collect();

    let mut tls_config = rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(
            public_key_chain,
            rustls::PrivateKey(certificate.private_key),
        )?;

    tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    let tls_config = Arc::new(tls_config);

    let tls_acceptor = Arc::new(TlsAcceptor::from(tls_config));

    let mut certificates = certificate_resolver.certificates.write().await;
    certificates.insert(sni.into(), tls_acceptor.clone());

    let tls_stream = tls_acceptor.accept(tcp_stream).await?;

    Ok(tls_stream)
}
