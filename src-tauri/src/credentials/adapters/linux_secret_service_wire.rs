//! Dormant raw Secret Service client. All operations use a private connection
//! and the same non-interactive message policy.
#![cfg_attr(not(test), allow(dead_code))] // Factory activation is gated by #152.

#[path = "linux_secret_service_crypto.rs"]
mod crypto;

#[cfg(target_os = "linux")]
#[path = "."]
mod linux {
    use super::crypto::{ALGORITHM, CryptoError, Exchange, SessionKey};
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
    const COLLECTION: &str = "org.freedesktop.Secret.Collection";
    const PROMPT: &str = "org.freedesktop.Secret.Prompt";
    const SESSION_INTERFACE: &str = "org.freedesktop.Secret.Session";
    const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
    const DEADLINE: Duration = Duration::from_secs(5);
    const CLOSE_BUDGET: Duration = Duration::from_millis(125);

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

    #[allow(dead_code)] // This wire locator is opaque outside the client.
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

    pub(in super::super) fn session_address() -> WireResult<Address> {
        Address::session().map_err(|_| Failure::Unavailable.into())
    }

    pub(in super::super) fn unix_builder(address: Address) -> WireResult<Builder<'static>> {
        // Reject executable and remote transports on both connection paths.
        if !matches!(address.transport(), Transport::Unix(_)) {
            return Err(Failure::Unavailable.into());
        }
        Ok(Builder::address(address))
    }

    fn read_at(
        address: Address,
        attributes: &Attributes,
        deadline: Duration,
    ) -> WireResult<Option<SecretBytes>> {
        if attributes.is_empty() {
            return Err(Failure::Internal.into());
        }
        drive(deadline, async |end| {
            let read_by = end - CLOSE_BUDGET;
            let mut client = until(read_by, Client::connect(address)).await?;
            let result = until(read_by, client.read_exact(attributes)).await;
            client.close_session(end).await;
            result
        })
    }

    fn read_alias_at(
        address: Address,
        deadline: Duration,
    ) -> WireResult<Option<DefaultCollection>> {
        drive(deadline, async |end| {
            until(end, async {
                let client = Client::connect(address).await?;
                Ok(client.default_collection().await?.map(DefaultCollection))
            })
            .await
        })
    }

    fn drive<T>(
        timeout: Duration,
        work: impl AsyncFnOnce(Instant) -> WireResult<T>,
    ) -> WireResult<T> {
        ensure_worker_context()?;
        zbus::block_on(work(Instant::now() + timeout))
    }

    pub(in super::super) fn ensure_worker_context() -> WireResult<()> {
        if tokio::runtime::Handle::try_current().is_ok()
            || zruntime::SharedRuntime::running().is_some()
            || zruntime::LocalRuntime::running().is_some()
        {
            return Err(Failure::Unsupported.into());
        }
        Ok(())
    }

    pub(in super::super) async fn until<T>(
        deadline: Instant,
        work: impl Future<Output = WireResult<T>>,
    ) -> WireResult<T> {
        if Instant::now() >= deadline {
            return Err(Failure::Unavailable.into());
        }
        let runtime = zruntime::SharedRuntime::current().map_err(|_| Failure::Unavailable)?;
        runtime
            .timeout_at(deadline, work)
            .await
            .map_err(|_| WireError(Failure::Unavailable))?
    }

    struct Client {
        connection: pinned::Transport,
        owner: OwnedUniqueName,
        session: Option<OwnedObjectPath>,
    }

    // Keep the ordinary connection opaque even to sibling adapter modules.
    // They can use the closed routes, but cannot get a raw sender from Client.
    mod pinned {
        use super::*;
        use futures_util::StreamExt;
        use zbus::message::Type;
        use zbus::{MatchRule, MessageStream};

        pub(super) struct Transport(Connection);

        impl Transport {
            pub(super) fn new(connection: Connection) -> Self {
                Self(connection)
            }

            pub(super) fn server_guid(&self) -> &zbus::OwnedGuid {
                self.0.server_guid()
            }

            pub(super) async fn call<B, R>(
                &self,
                owner: &OwnedUniqueName,
                method: Method<'_>,
                body: &B,
            ) -> WireResult<R>
            where
                B: Serialize + DynamicType,
                R: for<'de> DynamicDeserialize<'de>,
            {
                flagged_call(&self.0, Some(owner), method, body).await
            }

            pub(super) async fn dismiss(
                &self,
                owner: &OwnedUniqueName,
                path: &OwnedObjectPath,
            ) -> WireResult<bool> {
                let rule = MatchRule::builder()
                    .msg_type(Type::Signal)
                    .sender(owner.as_str())
                    .path(path.as_str())
                    .interface(PROMPT)
                    .member("Completed")
                    .build()
                    .map_err(map_error)?;
                let mut stream = MessageStream::for_match_rule(rule, &self.0, None)
                    .await
                    .map_err(map_error)?;
                self.call::<_, ()>(owner, Method::Dismiss(path), &())
                    .await?;
                let completion = stream
                    .next()
                    .await
                    .ok_or(Failure::Unavailable)?
                    .map_err(map_error)?;
                let (dismissed, _): (bool, OwnedVariant) =
                    completion.body().deserialize().map_err(map_error)?;
                Ok(dismissed)
            }

            #[cfg(test)]
            pub(super) async fn disconnect(&self) {
                self.0.clone().close().await.unwrap();
            }
        }
    }

    #[derive(Clone, Copy)]
    enum Method<'a> {
        GetNameOwner,
        #[cfg(test)]
        GetUnixProcessId,
        #[cfg(test)]
        LockCollection,
        ReadAlias,
        SearchItems,
        OpenSession,
        ReadProperty(&'a OwnedObjectPath),
        GetSecret(&'a OwnedObjectPath),
        SetSecret(&'a OwnedObjectPath),
        CreateItem(&'a OwnedObjectPath),
        DeleteItem(&'a OwnedObjectPath),
        CloseSession(&'a OwnedObjectPath),
        Dismiss(&'a OwnedObjectPath),
    }

    impl Method<'_> {
        fn route(&self) -> (&str, &str, &str) {
            match self {
                Self::GetNameOwner => (BUS_PATH, BUS, "GetNameOwner"),
                #[cfg(test)]
                Self::GetUnixProcessId => (BUS_PATH, BUS, "GetConnectionUnixProcessID"),
                #[cfg(test)]
                Self::LockCollection => (SERVICE_PATH, SERVICE, "Lock"),
                Self::ReadAlias => (SERVICE_PATH, SERVICE, "ReadAlias"),
                Self::SearchItems => (SERVICE_PATH, SERVICE, "SearchItems"),
                Self::OpenSession => (SERVICE_PATH, SERVICE, "OpenSession"),
                Self::ReadProperty(path) => (path.as_str(), PROPERTIES, "Get"),
                Self::GetSecret(path) => (path.as_str(), ITEM, "GetSecret"),
                Self::SetSecret(path) => (path.as_str(), ITEM, "SetSecret"),
                Self::CreateItem(path) => (path.as_str(), COLLECTION, "CreateItem"),
                Self::DeleteItem(path) => (path.as_str(), ITEM, "Delete"),
                Self::CloseSession(path) => (path.as_str(), SESSION_INTERFACE, "Close"),
                Self::Dismiss(path) => (path.as_str(), PROMPT, "Dismiss"),
            }
        }
    }

    async fn name_owner(connection: &Connection, name: &str) -> WireResult<OwnedUniqueName> {
        flagged_call(connection, None, Method::GetNameOwner, &(name,)).await
    }

    async fn require_gnome_owner(
        connection: &Connection,
        pinned: &OwnedUniqueName,
    ) -> WireResult<()> {
        match name_owner(connection, "org.gnome.keyring").await {
            Ok(owner) if owner == *pinned => Ok(()),
            _ => Err(Failure::Unsupported.into()),
        }
    }

    impl Client {
        async fn connect(address: Address) -> WireResult<Self> {
            let connection = unix_builder(address)?
                .build()
                .await
                .map_err(|_| Failure::Unavailable)?;
            Self::bind(connection).await
        }

        async fn bind(connection: Connection) -> WireResult<Self> {
            // Pin the provider for the entire operation. A new owner cannot
            // answer a request intended for the old owner's item or session.
            let owner = name_owner(&connection, SERVICE_NAME)
                .await
                .map_err(unavailable_if_missing)?;
            Ok(Self {
                connection: pinned::Transport::new(connection),
                owner,
                session: None,
            })
        }

        async fn call<B, R>(&self, method: Method<'_>, body: &B) -> WireResult<R>
        where
            B: Serialize + DynamicType,
            R: for<'de> DynamicDeserialize<'de>,
        {
            self.connection
                .call(&self.owner, method, body)
                .await
                .map_err(|error| {
                    if method.route().1 == SERVICE {
                        unavailable_if_missing(error)
                    } else {
                        error
                    }
                })
        }

        async fn dismiss_mutation_prompt(&self, path: &OwnedObjectPath) -> WireResult<bool> {
            self.connection.dismiss(&self.owner, path).await
        }

        async fn property(&self, item: &OwnedObjectPath, name: &str) -> WireResult<OwnedVariant> {
            self.call(Method::ReadProperty(item), &(ITEM, name)).await
        }

        async fn attributes(&self, item: &OwnedObjectPath) -> WireResult<Attributes> {
            self.property(item, "Attributes")
                .await?
                .try_into()
                .map_err(|_| Failure::CorruptRecord.into())
        }

        async fn default_collection(&self) -> WireResult<Option<OwnedObjectPath>> {
            let alias: OwnedObjectPath = self.call(Method::ReadAlias, &("default",)).await?;
            Ok((alias.as_str() != "/").then_some(alias))
        }

        async fn find_exact(
            &self,
            attributes: &Attributes,
            allow_disappeared: bool,
        ) -> WireResult<Option<OwnedObjectPath>> {
            match self.exact_candidate(attributes, allow_disappeared).await? {
                Some((_, true)) => Err(Failure::Locked.into()),
                candidate => Ok(candidate.map(|(item, _)| item)),
            }
        }

        async fn exact_candidate(
            &self,
            attributes: &Attributes,
            allow_disappeared: bool,
        ) -> WireResult<Option<(OwnedObjectPath, bool)>> {
            let (unlocked, locked): SearchResults =
                self.call(Method::SearchItems, &(attributes,)).await?;
            let mut candidate = None;
            // SearchItems can return supersets of the requested attributes.
            // Preserve each candidate's search half while checking equality.
            for (items, was_locked) in [(unlocked, false), (locked, true)] {
                for item in items {
                    let actual = match self.attributes(&item).await {
                        Err(WireError(Failure::Missing)) if was_locked => {
                            return Err(Failure::Locked.into());
                        }
                        Err(WireError(Failure::Missing)) if allow_disappeared => continue,
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
            Ok(candidate)
        }

        async fn read_exact(&mut self, attributes: &Attributes) -> WireResult<Option<SecretBytes>> {
            let Some(item) = self.find_exact(attributes, true).await? else {
                return Ok(None);
            };
            match self.read_unlocked_candidate(&item, attributes).await {
                Ok(secret) => Ok(Some(secret)),
                Err(WireError(Failure::Missing)) => Ok(None),
                Err(error) => Err(error),
            }
        }

        async fn read_unlocked_candidate(
            &mut self,
            item: &OwnedObjectPath,
            attributes: &Attributes,
        ) -> WireResult<SecretBytes> {
            self.require_unlocked(item, ITEM).await?;
            let key = self.open_session().await?;
            self.get_secret(item, attributes, &key).await
        }

        async fn require_unlocked(
            &self,
            path: &OwnedObjectPath,
            interface: &str,
        ) -> WireResult<()> {
            if self.locked(path, interface).await? {
                return Err(Failure::Locked.into());
            }
            Ok(())
        }

        async fn locked(&self, path: &OwnedObjectPath, interface: &str) -> WireResult<bool> {
            let locked: OwnedVariant = self
                .call(Method::ReadProperty(path), &(interface, "Locked"))
                .await?;
            bool::try_from(locked).map_err(|_| Failure::CorruptRecord.into())
        }

        async fn open_session(&mut self) -> WireResult<SessionKey> {
            let exchange = Exchange::generate().map_err(map_crypto_error)?;
            let public_key = Variant::from(exchange.public_key().to_vec());
            let (peer_key, session): (OwnedVariant, OwnedObjectPath) = self
                .call(Method::OpenSession, &(ALGORITHM, public_key))
                .await?;
            if session.as_str() == "/" {
                return Err(Failure::Unsupported.into());
            }
            self.session = Some(session);
            if matches!(&*peer_key, Variant::Str(_)) {
                return Err(Failure::Unsupported.into());
            }
            let peer_key = Vec::<u8>::try_from(peer_key).map_err(|_| Failure::CorruptRecord)?;
            exchange
                .finish(&peer_key)
                .map_err(|_| Failure::CorruptRecord.into())
        }

        fn session_path(&self) -> WireResult<&OwnedObjectPath> {
            self.session
                .as_ref()
                .ok_or_else(|| Failure::Internal.into())
        }

        async fn get_secret(
            &self,
            item: &OwnedObjectPath,
            attributes: &Attributes,
            key: &SessionKey,
        ) -> WireResult<SecretBytes> {
            let session = self.session_path()?;
            let (reply_session, iv, encrypted, _content_type): SecretReply =
                self.call(Method::GetSecret(item), &(session,)).await?;
            if &reply_session != session {
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

        async fn close_session(&mut self, by: Instant) {
            if let Some(session) = self.session.take() {
                // Preserve the outcome if cleanup fails. Disconnect also retires
                // the session when the transport is still usable by the peer.
                let _: WireResult<()> =
                    until(by, self.call(Method::CloseSession(&session), &())).await;
            }
        }
    }

    fn map_crypto_error(error: CryptoError) -> WireError {
        match error {
            CryptoError::EntropyUnavailable => Failure::Unavailable,
            _ => Failure::Internal,
        }
        .into()
    }

    async fn flagged_call<B, R>(
        connection: &Connection,
        owner: Option<&OwnedUniqueName>,
        method: Method<'_>,
        body: &B,
    ) -> WireResult<R>
    where
        B: Serialize + DynamicType,
        R: for<'de> DynamicDeserialize<'de>,
    {
        let (path, interface, member) = method.route();
        let destination = if interface == BUS {
            BUS
        } else {
            owner.ok_or(Failure::Internal)?.as_str()
        };
        let proxy: Proxy<'_> = ProxyBuilder::new(connection)
            .destination(destination)
            .path(path)
            .interface(interface)
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .map_err(map_error)?;
        proxy
            .call_with_flags(member, MethodFlags::NO_AUTO_START, body)
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

    pub(in super::super) fn map_error(error: zbus::Error) -> WireError {
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

    #[path = "linux_secret_service_mutations.rs"]
    mod mutations;
    #[path = "linux_secret_service_recovery_target.rs"]
    mod recovery_target;
    #[cfg(test)]
    #[path = "linux_secret_service_wire_tests.rs"]
    pub(in super::super) mod tests;
    pub(in super::super) use mutations::{MutationOutcome, delete_exact, write_exact};
    pub(in super::super) use recovery_target::{LockedTarget, locked_target};
    #[cfg(test)]
    pub(in super::super) use recovery_target::{attributes_for, locked_target_at};
}

#[cfg(all(target_os = "linux", test))]
pub(super) use linux::locked_target_at;
#[cfg(all(target_os = "linux", test))]
pub(super) use linux::tests as scripted_peer;
#[cfg(target_os = "linux")]
#[allow(unused_imports)] // The factory remains dormant.
pub(super) use linux::{
    LockedTarget, WireError, ensure_worker_context, locked_target, map_error as recovery_error,
    session_address, unix_builder, until as recovery_until,
};
#[cfg(target_os = "linux")]
#[allow(unused_imports)] // The factory remains dormant.
pub(super) use linux::{MutationOutcome, delete_exact, write_exact};
#[cfg(target_os = "linux")]
#[allow(unused_imports)] // The factory remains dormant.
pub(super) use linux::{read_default_alias, read_exact};
