use std::{error::Error, path::Path, sync::Arc};

use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};

pub fn create_tls_acceptor(
    cert_path: impl AsRef<Path>,
    key_path: impl AsRef<Path>,
) -> Result<TlsAcceptor, Box<dyn Error + Send + Sync>> {
    let certs = CertificateDer::pem_file_iter(cert_path)?.collect::<Result<Vec<_>, _>>()?;

    if certs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Certificate chain is empty",
        )
        .into());
    }

    let private_key = PrivateKeyDer::from_pem_file(key_path)?;
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, private_key)?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}
