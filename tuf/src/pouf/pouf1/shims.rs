use {
    crate::{
        crypto,
        error::Error,
        metadata::{self, Metadata},
        Result,
    },
    chrono::{offset::Utc, prelude::*},
    serde_derive::{Deserialize, Serialize},
    std::{
        collections::{BTreeMap, HashSet},
        marker::PhantomData,
    },
};

const SPEC_VERSION: &str = "1.0";

// Ensure the given spec version matches our spec version.
//
// We also need to handle the literal "1.0" here, despite that fact that it is not a valid version
// according to the SemVer spec, because it is already baked into some of the old roots.
fn valid_spec_version(other: &str) -> bool {
    matches!(other, "1.0" | "1.0.0")
}

fn parse_datetime(ts: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(ts)
        .map(|ts| ts.with_timezone(&Utc))
        .map_err(|e| Error::Encoding(format!("Can't parse DateTime: {:?}", e)))
}

fn format_datetime(ts: &DateTime<Utc>) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        ts.year(),
        ts.month(),
        ts.day(),
        ts.hour(),
        ts.minute(),
        ts.second()
    )
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RootMetadata {
    #[serde(rename = "_type")]
    typ: metadata::Role,
    spec_version: String,
    version: u32,
    consistent_snapshot: bool,
    expires: String,
    #[serde(deserialize_with = "deserialize_reject_duplicates::deserialize")]
    keys: BTreeMap<crypto::KeyId, crypto::PublicKey>,
    roles: RoleDefinitions,
}

impl RootMetadata {
    pub fn from(meta: &metadata::RootMetadata) -> Result<Self> {
        Ok(RootMetadata {
            typ: metadata::Role::Root,
            spec_version: SPEC_VERSION.to_string(),
            version: meta.version(),
            expires: format_datetime(meta.expires()),
            consistent_snapshot: meta.consistent_snapshot(),
            keys: meta
                .keys()
                .iter()
                .map(|(id, key)| (id.clone(), key.clone()))
                .collect(),
            roles: RoleDefinitions {
                root: meta.root().clone(),
                snapshot: meta.snapshot().clone(),
                targets: meta.targets().clone(),
                timestamp: meta.timestamp().clone(),
            },
        })
    }

    pub fn try_into(self) -> Result<metadata::RootMetadata> {
        if self.typ != metadata::Role::Root {
            return Err(Error::Encoding(format!(
                "Attempted to decode root metadata labeled as {:?}",
                self.typ
            )));
        }

        if !valid_spec_version(&self.spec_version) {
            return Err(Error::Encoding(format!(
                "Unknown spec version {}",
                self.spec_version
            )));
        }

        // Ignore all keys with incorrect key IDs. We should give an error if the key ID is not
        // correct according to TUF spec. However, due to backward compatibility, we may receive
        // metadata with key IDs generated by TUF 0.9. We simply ignore those old keys.
        let keys_with_correct_key_id = self
            .keys
            .into_iter()
            .filter(|(key_id, pkey)| key_id == pkey.key_id())
            .collect();

        metadata::RootMetadata::new(
            self.version,
            parse_datetime(&self.expires)?,
            self.consistent_snapshot,
            keys_with_correct_key_id,
            self.roles.root,
            self.roles.snapshot,
            self.roles.targets,
            self.roles.timestamp,
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RoleDefinitions {
    root: metadata::RoleDefinition<metadata::RootMetadata>,
    snapshot: metadata::RoleDefinition<metadata::SnapshotMetadata>,
    targets: metadata::RoleDefinition<metadata::TargetsMetadata>,
    timestamp: metadata::RoleDefinition<metadata::TimestampMetadata>,
}

#[derive(Serialize, Deserialize)]
pub struct RoleDefinition<M: Metadata> {
    threshold: u32,
    #[serde(rename = "keyids")]
    key_ids: Vec<crypto::KeyId>,
    #[serde(skip)]
    _metadata: PhantomData<M>,
}

impl<M: Metadata> From<&metadata::RoleDefinition<M>> for RoleDefinition<M> {
    fn from(role: &metadata::RoleDefinition<M>) -> Self {
        // Sort the key ids so they're in a stable order.
        let mut key_ids = role.key_ids().iter().cloned().collect::<Vec<_>>();
        key_ids.sort();

        RoleDefinition {
            threshold: role.threshold(),
            key_ids,
            _metadata: PhantomData,
        }
    }
}

impl<M: Metadata> TryFrom<RoleDefinition<M>> for metadata::RoleDefinition<M> {
    type Error = Error;

    fn try_from(definition: RoleDefinition<M>) -> Result<Self> {
        let key_ids_len = definition.key_ids.len();
        let mut key_ids = HashSet::with_capacity(key_ids_len);

        for key_id in definition.key_ids {
            if let Some(old_key_id) = key_ids.replace(key_id) {
                return Err(Error::MetadataRoleHasDuplicateKeyId {
                    role: M::ROLE.into(),
                    key_id: old_key_id,
                });
            }
        }

        metadata::RoleDefinition::new(definition.threshold, key_ids)
    }
}

#[derive(Serialize, Deserialize)]
pub struct TimestampMetadata {
    #[serde(rename = "_type")]
    typ: metadata::Role,
    spec_version: String,
    version: u32,
    expires: String,
    meta: TimestampMeta,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimestampMeta {
    #[serde(rename = "snapshot.json")]
    snapshot: metadata::MetadataDescription<metadata::SnapshotMetadata>,
}

impl TimestampMetadata {
    pub fn from(metadata: &metadata::TimestampMetadata) -> Result<Self> {
        Ok(TimestampMetadata {
            typ: metadata::Role::Timestamp,
            spec_version: SPEC_VERSION.to_string(),
            version: metadata.version(),
            expires: format_datetime(metadata.expires()),
            meta: TimestampMeta {
                snapshot: metadata.snapshot().clone(),
            },
        })
    }

    pub fn try_into(self) -> Result<metadata::TimestampMetadata> {
        if self.typ != metadata::Role::Timestamp {
            return Err(Error::Encoding(format!(
                "Attempted to decode timestamp metadata labeled as {:?}",
                self.typ
            )));
        }

        if !valid_spec_version(&self.spec_version) {
            return Err(Error::Encoding(format!(
                "Unknown spec version {}",
                self.spec_version
            )));
        }

        metadata::TimestampMetadata::new(
            self.version,
            parse_datetime(&self.expires)?,
            self.meta.snapshot,
        )
    }
}

#[derive(Serialize, Deserialize)]
pub struct SnapshotMetadata {
    #[serde(rename = "_type")]
    typ: metadata::Role,
    spec_version: String,
    version: u32,
    expires: String,
    #[serde(deserialize_with = "deserialize_reject_duplicates::deserialize")]
    meta: BTreeMap<String, metadata::MetadataDescription<metadata::TargetsMetadata>>,
}

impl SnapshotMetadata {
    pub fn from(metadata: &metadata::SnapshotMetadata) -> Result<Self> {
        Ok(SnapshotMetadata {
            typ: metadata::Role::Snapshot,
            spec_version: SPEC_VERSION.to_string(),
            version: metadata.version(),
            expires: format_datetime(metadata.expires()),
            meta: metadata
                .meta()
                .iter()
                .map(|(p, d)| (format!("{}.json", p), d.clone()))
                .collect(),
        })
    }

    pub fn try_into(self) -> Result<metadata::SnapshotMetadata> {
        if self.typ != metadata::Role::Snapshot {
            return Err(Error::Encoding(format!(
                "Attempted to decode snapshot metadata labeled as {:?}",
                self.typ
            )));
        }

        if !valid_spec_version(&self.spec_version) {
            return Err(Error::Encoding(format!(
                "Unknown spec version {}",
                self.spec_version
            )));
        }

        metadata::SnapshotMetadata::new(
            self.version,
            parse_datetime(&self.expires)?,
            self.meta
                .into_iter()
                .map(|(p, d)| {
                    if !p.ends_with(".json") {
                        return Err(Error::Encoding(format!(
                            "Metadata does not end with .json: {}",
                            p
                        )));
                    }

                    let s = p.split_at(p.len() - ".json".len()).0.to_owned();
                    let p = metadata::MetadataPath::new(s)?;

                    Ok((p, d))
                })
                .collect::<Result<_>>()?,
        )
    }
}

#[derive(Serialize, Deserialize)]
pub struct TargetsMetadata {
    #[serde(rename = "_type")]
    typ: metadata::Role,
    spec_version: String,
    version: u32,
    expires: String,
    targets: BTreeMap<metadata::TargetPath, metadata::TargetDescription>,
    #[serde(default, skip_serializing_if = "metadata::Delegations::is_empty")]
    delegations: metadata::Delegations,
}

impl TargetsMetadata {
    pub fn from(metadata: &metadata::TargetsMetadata) -> Result<Self> {
        Ok(TargetsMetadata {
            typ: metadata::Role::Targets,
            spec_version: SPEC_VERSION.to_string(),
            version: metadata.version(),
            expires: format_datetime(metadata.expires()),
            targets: metadata
                .targets()
                .iter()
                .map(|(p, d)| (p.clone(), d.clone()))
                .collect(),
            delegations: metadata.delegations().clone(),
        })
    }

    pub fn try_into(self) -> Result<metadata::TargetsMetadata> {
        if self.typ != metadata::Role::Targets {
            return Err(Error::Encoding(format!(
                "Attempted to decode targets metadata labeled as {:?}",
                self.typ
            )));
        }

        if !valid_spec_version(&self.spec_version) {
            return Err(Error::Encoding(format!(
                "Unknown spec version {}",
                self.spec_version
            )));
        }

        metadata::TargetsMetadata::new(
            self.version,
            parse_datetime(&self.expires)?,
            self.targets.into_iter().collect(),
            self.delegations,
        )
    }
}

#[derive(Serialize, Deserialize)]
pub struct PublicKey {
    keytype: crypto::KeyType,
    scheme: crypto::SignatureScheme,
    #[serde(skip_serializing_if = "Option::is_none")]
    keyid_hash_algorithms: Option<Vec<String>>,
    keyval: PublicKeyValue,
}

impl PublicKey {
    pub fn new(
        keytype: crypto::KeyType,
        scheme: crypto::SignatureScheme,
        keyid_hash_algorithms: Option<Vec<String>>,
        public_key: String,
    ) -> Self {
        PublicKey {
            keytype,
            scheme,
            keyid_hash_algorithms,
            keyval: PublicKeyValue { public: public_key },
        }
    }

    pub fn public_key(&self) -> &str {
        &self.keyval.public
    }

    pub fn scheme(&self) -> &crypto::SignatureScheme {
        &self.scheme
    }

    pub fn keytype(&self) -> &crypto::KeyType {
        &self.keytype
    }

    pub fn keyid_hash_algorithms(&self) -> &Option<Vec<String>> {
        &self.keyid_hash_algorithms
    }
}

#[derive(Serialize, Deserialize)]
pub struct PublicKeyValue {
    public: String,
}

#[derive(Serialize, Deserialize)]
pub struct Delegation {
    name: metadata::MetadataPath,
    terminating: bool,
    threshold: u32,
    #[serde(rename = "keyids")]
    key_ids: Vec<crypto::KeyId>,
    paths: Vec<metadata::TargetPath>,
}

impl From<&metadata::Delegation> for Delegation {
    fn from(delegation: &metadata::Delegation) -> Self {
        let mut paths = delegation
            .paths()
            .iter()
            .cloned()
            .collect::<Vec<metadata::TargetPath>>();
        paths.sort();

        let mut key_ids = delegation
            .key_ids()
            .iter()
            .cloned()
            .collect::<Vec<crypto::KeyId>>();
        key_ids.sort();

        Delegation {
            name: delegation.name().clone(),
            terminating: delegation.terminating(),
            threshold: delegation.threshold(),
            key_ids,
            paths,
        }
    }
}

impl TryFrom<Delegation> for metadata::Delegation {
    type Error = Error;

    fn try_from(delegation: Delegation) -> Result<Self> {
        let delegation_key_ids_len = delegation.key_ids.len();
        let key_ids = delegation.key_ids.into_iter().collect::<HashSet<_>>();

        if key_ids.len() != delegation_key_ids_len {
            return Err(Error::Encoding("Non-unique delegation key IDs.".into()));
        }

        let delegation_paths_len = delegation.paths.len();
        let paths = delegation.paths.into_iter().collect::<HashSet<_>>();

        if paths.len() != delegation_paths_len {
            return Err(Error::Encoding("Non-unique delegation paths.".into()));
        }

        metadata::Delegation::new(
            delegation.name,
            delegation.terminating,
            delegation.threshold,
            key_ids,
            paths,
        )
    }
}

#[derive(Serialize, Deserialize)]
pub struct Delegations {
    #[serde(deserialize_with = "deserialize_reject_duplicates::deserialize")]
    keys: BTreeMap<crypto::KeyId, crypto::PublicKey>,
    roles: Vec<Delegation>,
}

impl From<&metadata::Delegations> for Delegations {
    fn from(delegations: &metadata::Delegations) -> Delegations {
        let mut roles = delegations
            .roles()
            .iter()
            .map(Delegation::from)
            .collect::<Vec<Delegation>>();

        // We want our roles in a consistent order.
        roles.sort_by(|lhs, rhs| lhs.name.cmp(&rhs.name));

        Delegations {
            keys: delegations
                .keys()
                .iter()
                .map(|(id, key)| (id.clone(), key.clone()))
                .collect(),
            roles,
        }
    }
}

impl TryFrom<Delegations> for metadata::Delegations {
    type Error = Error;

    fn try_from(delegations: Delegations) -> Result<metadata::Delegations> {
        metadata::Delegations::new(
            delegations.keys.into_iter().collect(),
            delegations
                .roles
                .into_iter()
                .map(|delegation| delegation.try_into())
                .collect::<Result<Vec<_>>>()?,
        )
    }
}

#[derive(Serialize, Deserialize)]
pub struct TargetDescription {
    length: u64,
    hashes: BTreeMap<crypto::HashAlgorithm, crypto::HashValue>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    custom: BTreeMap<String, serde_json::Value>,
}

impl From<&metadata::TargetDescription> for TargetDescription {
    fn from(description: &metadata::TargetDescription) -> TargetDescription {
        TargetDescription {
            length: description.length(),
            hashes: description
                .hashes()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            custom: description
                .custom()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
}

impl TryFrom<TargetDescription> for metadata::TargetDescription {
    type Error = Error;

    fn try_from(description: TargetDescription) -> Result<Self> {
        metadata::TargetDescription::new(
            description.length,
            description.hashes.into_iter().collect(),
            description.custom.into_iter().collect(),
        )
    }
}

#[derive(Serialize, Deserialize)]
pub struct MetadataDescription<M: Metadata> {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    length: Option<usize>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    hashes: BTreeMap<crypto::HashAlgorithm, crypto::HashValue>,
    #[serde(skip)]
    _metadata: PhantomData<M>,
}

impl<M: Metadata> From<&metadata::MetadataDescription<M>> for MetadataDescription<M> {
    fn from(description: &metadata::MetadataDescription<M>) -> Self {
        Self {
            version: description.version(),
            length: description.length(),
            hashes: description
                .hashes()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            _metadata: PhantomData,
        }
    }
}

impl<M: Metadata> TryFrom<MetadataDescription<M>> for metadata::MetadataDescription<M> {
    type Error = Error;

    fn try_from(description: MetadataDescription<M>) -> Result<Self> {
        metadata::MetadataDescription::new(
            description.version,
            description.length,
            description.hashes.into_iter().collect(),
        )
    }
}

/// Custom deserialize to reject duplicate keys.
mod deserialize_reject_duplicates {
    use serde::de::{Deserialize, Deserializer, Error, MapAccess, Visitor};
    use std::collections::BTreeMap;
    use std::fmt;
    use std::marker::PhantomData;
    use std::result::Result;

    pub fn deserialize<'de, K, V, D>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        struct BTreeVisitor<K, V> {
            marker: PhantomData<(K, V)>,
        }

        impl<'de, K, V> Visitor<'de> for BTreeVisitor<K, V>
        where
            K: Deserialize<'de> + Ord,
            V: Deserialize<'de>,
        {
            type Value = BTreeMap<K, V>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("map")
            }

            fn visit_map<M>(self, mut access: M) -> std::result::Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut map = BTreeMap::new();
                while let Some((key, value)) = access.next_entry()? {
                    if map.insert(key, value).is_some() {
                        return Err(M::Error::custom("Cannot have duplicate keys"));
                    }
                }
                Ok(map)
            }
        }

        deserializer.deserialize_map(BTreeVisitor {
            marker: PhantomData,
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn spec_version_validation() {
        let valid_spec_versions = ["1.0.0", "1.0"];

        for version in valid_spec_versions {
            assert!(valid_spec_version(version), "{:?} should be valid", version);
        }

        let invalid_spec_versions = ["1.0.1", "1.1.0", "2.0.0", "3.0"];

        for version in invalid_spec_versions {
            assert!(
                !valid_spec_version(version),
                "{:?} should be invalid",
                version
            );
        }
    }

    #[test]
    fn datetime_formats() {
        // The TUF spec says datetimes should be in ISO8601 format, specifically
        // "YYYY-MM-DDTHH:MM:SSZ". Since not all TUF clients adhere strictly to that, we choose to
        // be more lenient here. The following represent the intersection of valid ISO8601 and
        // RFC3339 datetime formats (source: https://ijmacd.github.io/rfc3339-iso8601/).
        let valid_formats = [
            "2022-08-30T19:53:55Z",
            "2022-08-30T19:53:55.7Z",
            "2022-08-30T19:53:55.77Z",
            "2022-08-30T19:53:55.775Z",
            "2022-08-30T19:53:55+00:00",
            "2022-08-30T19:53:55.7+00:00",
            "2022-08-30T14:53:55-05:00",
            "2022-08-30T14:53:55.7-05:00",
            "2022-08-30T14:53:55.77-05:00",
            "2022-08-30T14:53:55.775-05:00",
        ];

        for format in valid_formats {
            assert!(parse_datetime(format).is_ok(), "should parse {:?}", format);
        }
    }
}
