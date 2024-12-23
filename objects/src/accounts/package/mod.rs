use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::{String, ToString},
    vec::Vec,
};

use assembly::Library;
use semver::Version;
use serde::{
    de::{Error as DeError, Unexpected},
    Deserialize, Deserializer, Serialize, Serializer,
};
use storage_entry::{TemplateKey, TemplateValue};
use thiserror::Error;
use vm_core::utils::{Deserializable, Serializable};

use super::{AccountComponent, AccountType};
use crate::AccountError;

mod storage_entry;
pub use storage_entry::StorageEntry;

// COMPONENT PACKAGE
// ================================================================================================

/// Represents a package containing a component's metadata and its associated library.
///
/// The [ComponentPackage] encapsulates all necessary information to initialize and manage
/// a component within the system. It includes the configuration details and the compiled
/// library code required for the component's operation.
///
/// A package can be instantiated into [AccountComponent] objects by calling `instantiate()`.
/// The component metadata can be defined with generic keys that can be replaced at instantiation
/// time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentPackage {
    /// The component's metadata. This describes the component and how the storage is laid out,
    /// alongside how storage values are initialized.
    metadata: ComponentMetadata,
    /// The account's previously-assembled code. This defines all functionality related to the
    /// component.
    library: Library,
}

impl ComponentPackage {
    /// Create a [ComponentPackage]
    pub fn new(
        metadata: ComponentMetadata,
        library: Library,
    ) -> Result<Self, ComponentPackageError> {
        _ = toml::to_string(&metadata)
            .map_err(|err| ComponentPackageError::MetadataDeserializationError(err.to_string()));
        Ok(Self { metadata, library })
    }

    /// Instantiates an [AccountComponent] from the [ComponentPackage].
    ///
    /// The package's component metadata might contain templated values, which can be input by
    /// mapping key names to [template values](TemplateValue) through the `template_keys`
    /// parameter.
    ///
    /// # Errors
    ///
    /// - If any of the component's storage entries cannot be transformed into a valid storage
    /// slot. This could be because the metadata is invalid, or template values were not provided
    /// (or they are not of a valid type)
    pub fn instantiate_component(
        &self,
        template_keys: &BTreeMap<String, TemplateValue>,
    ) -> Result<AccountComponent, ComponentPackageError> {
        let mut storage_slots = vec![];
        for storage_entry in self.metadata().storage_entries() {
            let entry_storage_slots =
                storage_entry.clone().try_into_storage_slots(template_keys)?;
            storage_slots.extend(entry_storage_slots);
        }

        AccountComponent::new(self.library.clone(), storage_slots)
            .map_err(ComponentPackageError::AccountComponentError)
    }
    pub fn metadata(&self) -> &ComponentMetadata {
        &self.metadata
    }

    pub fn library(&self) -> &Library {
        &self.library
    }
}

impl Serializable for ComponentPackage {
    fn write_into<W: vm_core::utils::ByteWriter>(&self, target: &mut W) {
        // Since `ComponentConfig::new` ensures valid TOML, unwrap is safe here.
        let config_toml =
            toml::to_string(&self.metadata).expect("Failed to serialize ComponentConfig to TOML");
        target.write(config_toml);
        target.write(&self.library);
    }
}

impl Deserializable for ComponentPackage {
    fn read_from<R: vm_core::utils::ByteReader>(
        source: &mut R,
    ) -> Result<Self, vm_processor::DeserializationError> {
        // Read and deserialize the configuration from a TOML string.
        let config_str = String::read_from(source)?;
        let config: ComponentMetadata = toml::from_str(&config_str)
            .map_err(|e| vm_processor::DeserializationError::InvalidValue(e.to_string()))?;
        let library = Library::read_from(source)?;

        let package = ComponentPackage::new(config, library).map_err(|err| {
            vm_processor::DeserializationError::UnknownError(format!(
                "error deserializing into a ComponentPackage: {}",
                err
            ))
        })?;
        Ok(package)
    }
}

// COMPONENT METADATA
// ================================================================================================

/// Represents the full component template configuration.
///
/// This struct allows for serialization and deserialization to and from a TOML file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentMetadata {
    /// The human-readable name of the component.
    name: String,

    /// A brief description of what this component is and how it works.
    description: String,

    /// The version of the component using semantic versioning.
    /// This can be used to track and manage component upgrades.
    version: Version,

    /// A set of supported target account types for this component.
    targets: BTreeSet<AccountType>,

    /// A list of storage entries defining the component's storage layout and initialization
    /// values.
    storage: Vec<StorageEntry>,
}

impl ComponentMetadata {
    /// Create a new `ComponentMetadata`.
    ///
    /// # Errors
    ///
    /// - If the specified storage slots are not contiguous across all storage entries.
    pub fn new(
        name: String,
        description: String,
        version: Version,
        targets: BTreeSet<AccountType>,
        storage: Vec<StorageEntry>,
    ) -> Result<Self, ComponentPackageError> {
        let component = Self {
            name,
            description,
            version,
            targets,
            storage,
        };
        component.validate()?;
        Ok(component)
    }

    /// Validate the `ComponentMetadata` object.
    ///
    /// # Errors
    ///
    /// - If the specified storage slots contain duplicates.
    /// - If the first slot is not zero.
    /// - If the slots are not contiguous.
    pub fn validate(&self) -> Result<(), ComponentPackageError> {
        let mut all_slots: Vec<u8> = self
            .storage
            .iter()
            .flat_map(|entry| entry.slot_indices().iter().copied())
            .collect();

        // Check for duplicates
        let mut seen = BTreeSet::new();
        let duplicate =
            all_slots
                .iter()
                .find_map(|&slot| if !seen.insert(slot) { Some(slot) } else { None });

        if let Some(dup) = duplicate {
            return Err(ComponentPackageError::DuplicateSlots(dup));
        }

        // Check that slots start at 0 and are contiguous
        all_slots.sort_unstable();
        if let Some(&first_slot) = all_slots.first() {
            if first_slot != 0 {
                return Err(ComponentPackageError::IncorrectStorageFirstSlot);
            }
        }
        for slots in all_slots.windows(2) {
            if slots[1] != slots[0] + 1 {
                return Err(ComponentPackageError::NonContiguousSlots);
            }
        }

        Ok(())
    }

    /// Deserializes `toml_string` and validates the resulting [ComponentMetadata]
    ///
    /// # Errors
    ///
    /// - If deserialization or validation fails
    pub fn from_toml(toml_string: &str) -> Result<Self, ComponentPackageError> {
        let component: ComponentMetadata =
            toml::from_str(toml_string).map_err(ComponentPackageError::DeserializationError)?;
        component.validate()?;
        Ok(component)
    }

    /// Retrieves the set of keys (identified by a string) that require a value at the moment of
    /// component instantiation.
    fn _get_template_keys(&self) -> BTreeSet<TemplateKey> {
        let mut key_set = BTreeSet::new();
        for storage_entry in &self.storage {
            for key in storage_entry.template_keys() {
                key_set.insert(key.clone());
            }
        }
        key_set
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    pub fn targets(&self) -> &BTreeSet<AccountType> {
        &self.targets
    }

    pub fn storage_entries(&self) -> &Vec<StorageEntry> {
        &self.storage
    }
}

#[derive(Debug, Error)]
pub enum ComponentPackageError {
    #[error("error creating AccountComponent: {0}")]
    AccountComponentError(AccountError),
    #[error("error trying to deserialize from toml")]
    DeserializationError(#[source] toml::de::Error),
    #[error("slot {0} is defined multiple times")]
    DuplicateSlots(u8),
    #[error("component storage slots have to start at 0")]
    IncorrectStorageFirstSlot,
    #[error("template value was not of the expected type {0}")]
    IncorrectTemplateValue(String),
    #[error("multi-slot entry should contain as many values as storage slots indices")]
    InvalidMultiSlotEntry,
    #[error("error deserializing component metadata: {0}")]
    MetadataDeserializationError(String),
    #[error("component storage slots are not contiguous")]
    NonContiguousSlots,
    #[error("error creating storage map: {0}")]
    StorageMapError(AccountError),
    #[error("template value ({0}) was not provided in the map")]
    TemplateValueNotProvided(String),
}

// SERIALIZATION
// ================================================================================================

impl Serialize for AccountType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = match self {
            AccountType::FungibleFaucet => "FungibleFaucet",
            AccountType::NonFungibleFaucet => "NonFungibleFaucet",
            AccountType::RegularAccountImmutableCode => "RegularAccountImmutableCode",
            AccountType::RegularAccountUpdatableCode => "RegularAccountUpdatableCode",
        };
        serializer.serialize_str(s)
    }
}

impl<'de> Deserialize<'de> for AccountType {
    fn deserialize<D>(deserializer: D) -> Result<AccountType, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;

        match s.to_lowercase().as_str() {
            "fungiblefaucet" => Ok(AccountType::FungibleFaucet),
            "nonfungiblefaucet" => Ok(AccountType::NonFungibleFaucet),
            "regularaccountimmutablecode" => Ok(AccountType::RegularAccountImmutableCode),
            "regularaccountupdatablecode" => Ok(AccountType::RegularAccountUpdatableCode),
            other => Err(D::Error::invalid_value(Unexpected::Str(other), &"a valid account type")),
        }
    }
}

// TESTS
// ================================================================================================

#[cfg(test)]
mod tests {
    use assembly::Assembler;
    use storage_entry::WordRepresentation;

    use super::*;
    use crate::testing::account_code::CODE;

    #[test]
    fn test_contiguous_value_slots() {
        let storage = vec![
            StorageEntry::Value {
                name: "slot0".into(),
                description: None,
                slot: 0,
                value: WordRepresentation::SingleHex(Default::default()),
            },
            StorageEntry::MultiSlot {
                name: "slot1".into(),
                description: None,
                slots: vec![1, 2],
                values: vec![
                    WordRepresentation::Array(Default::default()),
                    WordRepresentation::SingleHex(Default::default()),
                ],
            },
        ];

        let original_config = ComponentMetadata::new(
            "test".into(),
            "desc".into(),
            Version::parse("0.1.0").unwrap(),
            BTreeSet::new(),
            storage,
        )
        .unwrap();

        let serialized = toml::to_string(&original_config).unwrap();
        let deserialized: ComponentMetadata = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized, original_config)
    }

    #[test]
    fn test_new_non_contiguous_value_slots() {
        let storage = vec![
            StorageEntry::Value {
                name: "slot0".into(),
                description: None,
                slot: 0,
                value: Default::default(),
            },
            StorageEntry::Value {
                name: "slot2".into(),
                description: None,
                slot: 2,
                value: Default::default(),
            },
        ];

        let result = ComponentMetadata::new(
            "test".into(),
            "desc".into(),
            Version::parse("0.1.0").unwrap(),
            BTreeSet::new(),
            storage,
        );
        assert!(matches!(result, Err(ComponentPackageError::NonContiguousSlots)));
    }

    #[test]
    fn test_binary_serde_roundtrip() {
        let storage = vec![
            StorageEntry::MultiSlot {
                name: "slot1".into(),
                description: None,
                slots: vec![1, 2],
                values: vec![
                    WordRepresentation::Array(Default::default()),
                    WordRepresentation::SingleHex(Default::default()),
                ],
            },
            StorageEntry::Value {
                name: "slot0".into(),
                description: None,
                slot: 0,
                value: WordRepresentation::SingleHex(Default::default()),
            },
        ];

        let component_template = ComponentMetadata::new(
            "test".into(),
            "desc".into(),
            Version::parse("0.1.0").unwrap(),
            BTreeSet::new(),
            storage,
        )
        .unwrap();

        let library = Assembler::default().assemble_library([CODE]).unwrap();
        let package = ComponentPackage::new(component_template, library).unwrap();
        _ = package.instantiate_component(&BTreeMap::new()).unwrap();

        let serialized = package.to_bytes();
        let deserialized = ComponentPackage::read_from_bytes(&serialized).unwrap();

        assert_eq!(deserialized, package)
    }
}
