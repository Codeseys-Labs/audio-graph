//! Dormant raw Secret Service client. All operations use a private connection
//! and the same non-interactive message policy.
#![cfg_attr(not(test), allow(dead_code))] // Factory activation is gated by #152.

#[path = "linux_secret_service_crypto.rs"]
mod crypto;

#[cfg(target_os = "linux")]
#[path = "."]
mod linux {
    use super::crypto::{ALGORITHM, Exchange};
    use crate::credentials::domain::CredentialStoreFailure as Failure;
    use serde::Serialize;
    use std::collections::BTreeMap;
    use std::fmt;
    use std::future::Future;
    use std::time::{Duration, Instant};
    use zbus::address::Transport;
    use zbus::connection::Builder;
    use zbus::names::OwnedUniqueName;
    use zbus::proxy::{Builder as ProxyBuilder, CacheProperties, MethodFlags};
    use zbus::wire::DynamicDeserialize;
    use zbus::{
        Address, Connection, DynamicType, OwnedObjectPath, OwnedVariant, Proxy, Variant, fdo,
    };
    use zeroize::Zeroizing;

    const BUS: &str = "org.freedesktop.DBus";
    const BUS_PATH: &str = "/org/freedesktop/DBus";
    const SERVICE: &str = "org.freedesktop.Secret.Service";
    const SERVICE_NAME: &str = "org.freedesktop.secrets";
    const SERVICE_PATH: &str = "/org/freedesktop/secrets";
    const ITEM: &str = "org.freedesktop.Secret.Item";
    const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
    const DEADLINE: Duration = Duration::from_secs(5);

    pub(in super::super) type Attributes = BTreeMap<String, String>;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(in super::super) struct WireError(pub(in super::super) Failure);

    impl fmt::Display for WireError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "Secret Service: {:?}", self.0)
        }
    }

    impl From<Failure> for WireError {
        fn from(failure: Failure) -> Self {
            Self(failure)
        }
    }

    pub(in super::super) struct SecretBytes(Zeroizing<Vec<u8>>);

    impl SecretBytes {
        pub(in super::super) fn into_bytes(self) -> Zeroizing<Vec<u8>> {
            self.0
        }
    }

    impl fmt::Debug for SecretBytes {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("SecretBytes([redacted])")
        }
    }

    #[allow(dead_code)] // The next slice uses the alias to address a mutation.
    pub(in super::super) struct DefaultCollection(OwnedObjectPath);

    type WireResult<T> = Result<T, WireError>;
    type SearchResults = (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>);
    type SecretReply = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

    /// Run only on the dedicated credential worker, never on an async task.
    /// A new connection is retired when this read completes or times out.
    #[allow(dead_code)] // The private-bus tests inject an address into read_at.
    pub(in super::super) fn read_exact(attributes: &Attributes) -> WireResult<Option<SecretBytes>> {
        read_at(session_address()?, attributes, DEADLINE)
    }

    /// Run only on the dedicated credential worker, never on an async task.
    #[allow(dead_code)] // This slice performs global exact reads.
    pub(in super::super) fn read_default_alias() -> WireResult<Option<DefaultCollection>> {
        read_alias_at(session_address()?, DEADLINE)
    }

    fn session_address() -> WireResult<Address> {
        Address::session().map_err(|_| Failure::Unavailable.into())
    }

    fn read_at(
        address: Address,
        attributes: &Attributes,
        deadline: Duration,
    ) -> WireResult<Option<SecretBytes>> {
        if attributes.is_empty() {
            return Err(Failure::Internal.into());
        }
        drive(deadline, async {
            Client::connect(address).await?.read_exact(attributes).await
        })
    }

    fn read_alias_at(
        address: Address,
        deadline: Duration,
    ) -> WireResult<Option<DefaultCollection>> {
        drive(deadline, async {
            let client = Client::connect(address).await?;
            let alias: OwnedObjectPath = client.service_call("ReadAlias", &("default",)).await?;
            Ok((alias.as_str() != "/").then_some(DefaultCollection(alias)))
        })
    }

    /// One wall-clock deadline covers connect, handshake, and every request.
    /// The timer drops the pending future and its connection on expiry.
    fn drive<T>(timeout: Duration, work: impl Future<Output = WireResult<T>>) -> WireResult<T> {
        let deadline = Instant::now() + timeout;
        zbus::block_on(async {
            let runtime = zruntime::SharedRuntime::current().map_err(|_| Failure::Unavailable)?;
            runtime
                .timeout_at(deadline, work)
                .await
                .map_err(|_| WireError(Failure::Unavailable))?
        })
    }

    struct Client {
        connection: Connection,
        owner: OwnedUniqueName,
    }

    impl Client {
        async fn connect(address: Address) -> WireResult<Self> {
            // Secret Service is a local session-bus protocol. This also prevents
            // a configured address from selecting an executable transport.
            if !matches!(address.transport(), Transport::Unix(_)) {
                return Err(Failure::Unavailable.into());
            }
            let connection = Builder::address(address)
                .build()
                .await
                .map_err(|_| Failure::Unavailable)?;
            // Pin the provider for the entire operation. A new owner cannot
            // answer a request intended for the old owner's item or session.
            let owner = flagged_call(
                &connection,
                BUS,
                BUS_PATH,
                BUS,
                "GetNameOwner",
                &(SERVICE_NAME,),
            )
            .await
            .map_err(unavailable_if_missing)?;
            Ok(Self { connection, owner })
        }

        async fn service_call<B, R>(&self, method: &str, body: &B) -> WireResult<R>
        where
            B: Serialize + DynamicType,
            R: for<'de> DynamicDeserialize<'de>,
        {
            self.call(SERVICE_PATH, SERVICE, method, body)
                .await
                .map_err(unavailable_if_missing)
        }

        async fn call<B, R>(
            &self,
            path: &str,
            interface: &str,
            method: &str,
            body: &B,
        ) -> WireResult<R>
        where
            B: Serialize + DynamicType,
            R: for<'de> DynamicDeserialize<'de>,
        {
            flagged_call(
                &self.connection,
                self.owner.as_str(),
                path,
                interface,
                method,
                body,
            )
            .await
        }

        async fn property(&self, item: &OwnedObjectPath, name: &str) -> WireResult<OwnedVariant> {
            self.call(item.as_str(), PROPERTIES, "Get", &(ITEM, name))
                .await
        }

        async fn attributes(&self, item: &OwnedObjectPath) -> WireResult<Attributes> {
            self.property(item, "Attributes")
                .await?
                .try_into()
                .map_err(|_| Failure::CorruptRecord.into())
        }

        async fn read_exact(&self, attributes: &Attributes) -> WireResult<Option<SecretBytes>> {
            let (unlocked, locked): SearchResults =
                self.service_call("SearchItems", &(attributes,)).await?;
            let mut candidate = None;
            // SearchItems can return supersets of the requested attributes.
            // Preserve each candidate's search half while checking equality.
            for (items, was_locked) in [(unlocked, false), (locked, true)] {
                for item in items {
                    let actual = match self.attributes(&item).await {
                        Err(WireError(Failure::Missing)) if was_locked => {
                            return Err(Failure::Locked.into());
                        }
                        Err(WireError(Failure::Missing)) => continue,
                        result => result?,
                    };
                    if &actual == attributes {
                        if candidate.is_some() {
                            return Err(Failure::AmbiguousMatch.into());
                        }
                        candidate = Some((item, was_locked));
                    }
                }
            }
            let Some((item, was_locked)) = candidate else {
                return Ok(None);
            };
            if was_locked {
                return Err(Failure::Locked.into());
            }
            match self.read_unlocked_candidate(&item, attributes).await {
                Ok(secret) => Ok(Some(secret)),
                Err(WireError(Failure::Missing)) => Ok(None),
                Err(error) => Err(error),
            }
        }

        async fn read_unlocked_candidate(
            &self,
            item: &OwnedObjectPath,
            attributes: &Attributes,
        ) -> WireResult<SecretBytes> {
            if bool::try_from(self.property(item, "Locked").await?)
                .map_err(|_| Failure::CorruptRecord)?
            {
                return Err(Failure::Locked.into());
            }

            let exchange = {
                let mut entropy = Zeroizing::new([0; 32]);
                getrandom::fill(entropy.as_mut()).map_err(|_| Failure::Unavailable)?;
                Exchange::new(&entropy).map_err(|_| Failure::Internal)?
            };
            let public_key = Variant::from(exchange.public_key().to_vec());
            let (peer_key, session): (OwnedVariant, OwnedObjectPath) = self
                .service_call("OpenSession", &(ALGORITHM, public_key))
                .await?;
            if session.as_str() == "/" || matches!(&*peer_key, Variant::Str(_)) {
                return Err(Failure::Unsupported.into());
            }
            let peer_key = Vec::<u8>::try_from(peer_key).map_err(|_| Failure::CorruptRecord)?;
            let key = exchange
                .finish(&peer_key)
                .map_err(|_| Failure::CorruptRecord)?;
            let (reply_session, iv, encrypted, _content_type): SecretReply = self
                .call(item.as_str(), ITEM, "GetSecret", &(&session,))
                .await?;
            if reply_session != session {
                return Err(Failure::CorruptRecord.into());
            }
            if self.attributes(item).await? != *attributes {
                return Err(Failure::RevisionConflict.into());
            }
            let secret = key
                .decrypt(&iv, encrypted)
                .map_err(|_| Failure::CorruptRecord)?;
            Ok(SecretBytes(secret))
        }
    }

    async fn flagged_call<B, R>(
        connection: &Connection,
        destination: &str,
        path: &str,
        interface: &str,
        method: &str,
        body: &B,
    ) -> WireResult<R>
    where
        B: Serialize + DynamicType,
        R: for<'de> DynamicDeserialize<'de>,
    {
        let proxy: Proxy<'_> = ProxyBuilder::new(connection)
            .destination(destination)
            .path(path)
            .interface(interface)
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .map_err(map_error)?;
        proxy
            .call_with_flags(method, MethodFlags::NO_AUTO_START, body)
            .await
            .map_err(map_error)?
            .ok_or_else(|| Failure::Internal.into())
    }

    fn unavailable_if_missing(error: WireError) -> WireError {
        match error {
            WireError(Failure::Missing) => Failure::Unavailable.into(),
            error => error,
        }
    }

    fn map_error(error: zbus::Error) -> WireError {
        use zbus::Error as Z;
        match error {
            Z::MethodError(name, _, _) => match name.as_str() {
                "org.freedesktop.Secret.Error.IsLocked" => Failure::Locked,
                "org.freedesktop.Secret.Error.NoSuchObject" => Failure::Missing,
                "org.freedesktop.Secret.Error.NoSession" => Failure::Unavailable,
                _ => Failure::Internal,
            },
            Z::FDO(error) => match *error {
                fdo::Error::UnknownObject(_) => Failure::Missing,
                fdo::Error::AccessDenied(_)
                | fdo::Error::AuthFailed(_)
                | fdo::Error::InteractiveAuthorizationRequired(_) => Failure::AccessDenied,
                fdo::Error::NotSupported(_) => Failure::Unsupported,
                fdo::Error::ServiceUnknown(_)
                | fdo::Error::NameHasNoOwner(_)
                | fdo::Error::NoReply(_)
                | fdo::Error::IOError(_)
                | fdo::Error::NoServer(_)
                | fdo::Error::Timeout(_)
                | fdo::Error::TimedOut(_)
                | fdo::Error::Disconnected(_) => Failure::Unavailable,
                _ => Failure::Internal,
            },
            Z::Connection(..) | Z::InputOutput(_) | Z::Handshake(_) => Failure::Unavailable,
            Z::IncorrectType | Z::SignatureMismatch(..) | Z::InvalidReply => Failure::CorruptRecord,
            _ => Failure::Internal,
        }
        .into()
    }

    #[cfg(test)]
    #[path = "linux_secret_service_wire_tests.rs"]
    mod tests;
}

#[cfg(target_os = "linux")]
#[allow(unused_imports)] // The factory remains dormant.
pub(super) use linux::{read_default_alias, read_exact};
