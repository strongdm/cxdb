// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, StoreError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryBundle {
    pub registry_version: u32,
    pub bundle_id: String,
    #[serde(default)]
    pub types: HashMap<String, TypeEntry>,
    #[serde(default)]
    pub enums: HashMap<String, HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeEntry {
    #[serde(default)]
    pub versions: HashMap<String, TypeVersion>,
}

/// Specifies a frontend renderer for displaying payloads of this type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RendererSpec {
    /// ESM URL to load the renderer from. Use "builtin:Name" for bundled renderers.
    pub esm_url: String,
    /// Named export from the ESM module (defaults to "default").
    #[serde(default)]
    pub component: Option<String>,
    /// Subresource Integrity hash for security (optional).
    #[serde(default)]
    pub integrity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeVersion {
    #[serde(default)]
    pub fields: HashMap<String, FieldDef>,
    /// Optional frontend renderer specification.
    #[serde(default)]
    pub renderer: Option<RendererSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default, rename = "enum")]
    pub enum_ref: Option<String>,
    #[serde(default, rename = "ref")]
    pub type_ref: Option<String>,
    #[serde(default)]
    pub optional: Option<bool>,
    #[serde(default)]
    pub items: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSpec {
    pub name: String,
    pub field_type: String,
    pub enum_ref: Option<String>,
    pub type_ref: Option<String>,
    pub optional: bool,
    pub items: Option<ItemsSpec>,
}

/// Specifies array item type - either a simple type string or a type reference
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemsSpec {
    /// Simple type like "string", "int64"
    Simple(String),
    /// Reference to another type like "cxdb:ToolCallItem"
    Ref(String),
}

#[derive(Debug, Clone)]
pub struct TypeVersionSpec {
    pub version: u32,
    pub fields: HashMap<u64, FieldSpec>,
    /// Optional frontend renderer specification (passed through from TypeVersion).
    pub renderer: Option<RendererSpec>,
}

#[derive(Debug, Clone)]
pub struct TypeSpec {
    pub versions: BTreeMap<u32, TypeVersionSpec>,
    pub tag_schema: HashMap<u64, FieldSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSignature {
    pub field_type: String,
    pub enum_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Registry {
    dir: PathBuf,
    bundles: HashMap<String, Vec<u8>>,
    types: HashMap<String, TypeSpec>,
    enums: HashMap<String, HashMap<String, String>>,
    last_bundle_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    Created,
    AlreadyExists,
}

impl Registry {
    pub fn open(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let mut registry = Self {
            dir: dir.to_path_buf(),
            bundles: HashMap::new(),
            types: HashMap::new(),
            enums: HashMap::new(),
            last_bundle_id: None,
        };

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(&path)?;
            let bundle: RegistryBundle = serde_json::from_slice(&bytes)
                .map_err(|e| StoreError::Corrupt(format!("invalid bundle json: {e}")))?;
            let bundle_id = bundle.bundle_id.clone();
            registry.ingest_bundle(bundle, &bytes, true)?;
            registry.bundles.insert(bundle_id.clone(), bytes);
            registry.last_bundle_id = Some(bundle_id);
        }

        Ok(registry)
    }

    pub fn last_bundle_id(&self) -> Option<String> {
        self.last_bundle_id.clone()
    }

    pub fn get_bundle(&self, bundle_id: &str) -> Option<&[u8]> {
        self.bundles.get(bundle_id).map(|b| b.as_slice())
    }

    pub fn put_bundle(&mut self, bundle_id: &str, raw: &[u8]) -> Result<PutOutcome> {
        if let Some(existing) = self.bundles.get(bundle_id) {
            if existing == raw {
                return Ok(PutOutcome::AlreadyExists);
            }
            return Err(StoreError::InvalidInput(
                "bundle_id already exists with different content".into(),
            ));
        }

        let bundle: RegistryBundle = serde_json::from_slice(raw)
            .map_err(|e| StoreError::InvalidInput(format!("invalid json: {e}")))?;
        if bundle.bundle_id != bundle_id {
            return Err(StoreError::InvalidInput(
                "bundle_id does not match path".into(),
            ));
        }

        self.ingest_bundle(bundle.clone(), raw, false)?;

        let filename = bundle_filename(bundle_id);
        let path = self.dir.join(filename);
        fs::write(&path, raw)?;

        self.bundles.insert(bundle_id.to_string(), raw.to_vec());
        self.last_bundle_id = Some(bundle_id.to_string());

        Ok(PutOutcome::Created)
    }

    pub fn get_type_version(&self, type_id: &str, version: u32) -> Option<&TypeVersionSpec> {
        self.types.get(type_id)?.versions.get(&version)
    }

    pub fn get_latest_type_version(&self, type_id: &str) -> Option<&TypeVersionSpec> {
        self.types
            .get(type_id)?
            .versions
            .iter()
            .next_back()
            .map(|(_, v)| v)
    }

    pub fn get_enum(&self, enum_id: &str) -> Option<&HashMap<String, String>> {
        self.enums.get(enum_id)
    }

    pub fn stats(&self) -> RegistryStats {
        RegistryStats {
            bundles_total: self.bundles.len(),
            types_total: self.types.len(),
            enums_total: self.enums.len(),
        }
    }

    /// Returns a mapping of type_id -> RendererSpec for all types with renderers.
    /// Uses the latest version's renderer for each type.
    pub fn get_all_renderers(&self) -> HashMap<String, RendererSpec> {
        let mut result = HashMap::new();
        for (type_id, type_spec) in &self.types {
            // Get the latest version's renderer (BTreeMap is ordered, last = highest version)
            if let Some((_, version_spec)) = type_spec.versions.iter().next_back() {
                if let Some(renderer) = &version_spec.renderer {
                    result.insert(type_id.clone(), renderer.clone());
                }
            }
        }
        result
    }

    fn ingest_bundle(&mut self, bundle: RegistryBundle, raw: &[u8], loading: bool) -> Result<()> {
        if bundle.registry_version == 0 {
            return Err(StoreError::InvalidInput(
                "registry_version must be > 0".into(),
            ));
        }

        // Merge enums
        for (enum_id, mapping) in bundle.enums.iter() {
            if let Some(existing) = self.enums.get(enum_id) {
                if existing != mapping {
                    return Err(StoreError::InvalidInput(format!(
                        "enum {enum_id} already exists with different mapping"
                    )));
                }
            } else {
                self.enums.insert(enum_id.clone(), mapping.clone());
            }
        }

        // Merge types
        for (type_id, type_entry) in bundle.types.iter() {
            let type_spec = self
                .types
                .entry(type_id.clone())
                .or_insert_with(|| TypeSpec {
                    versions: BTreeMap::new(),
                    tag_schema: HashMap::new(),
                });

            for (version_str, version_def) in type_entry.versions.iter() {
                let version = parse_version(version_str)?;
                let normalized = normalize_version(version, version_def)?;

                if let Some(existing) = type_spec.versions.get_mut(&version) {
                    if existing.fields != normalized.fields {
                        return Err(StoreError::InvalidInput(format!(
                            "type {type_id} version {version} differs from existing"
                        )));
                    }
                    // Fields match - check if we should update the renderer
                    if normalized.renderer.is_some() && existing.renderer.is_none() {
                        existing.renderer = normalized.renderer.clone();
                    }
                    continue;
                }

                for (tag, field) in normalized.fields.iter() {
                    let signature = FieldSignature {
                        field_type: field.field_type.clone(),
                        enum_ref: field.enum_ref.clone(),
                    };
                    if let Some(existing) = type_spec.tag_schema.get(tag) {
                        if existing != &signature {
                            return Err(StoreError::InvalidInput(format!(
                                "tag reuse conflict for type {type_id} tag {tag}"
                            )));
                        }
                    } else {
                        type_spec.tag_schema.insert(*tag, signature);
                    }
                }

                type_spec.versions.insert(version, normalized);
            }
        }

        // Validate enum references after merge
        for (type_id, type_spec) in self.types.iter() {
            for (version, version_spec) in type_spec.versions.iter() {
                for (tag, field) in version_spec.fields.iter() {
                    if let Some(enum_ref) = &field.enum_ref {
                        if !self.enums.contains_key(enum_ref) {
                            return Err(StoreError::InvalidInput(format!(
                                "missing enum {enum_ref} for type {type_id} version {version} tag {tag}"
                            )));
                        }
                    }
                }
            }
        }

        if !loading {
            let _ = raw;
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RegistryStats {
    pub bundles_total: usize,
    pub types_total: usize,
    pub enums_total: usize,
}

fn parse_version(version: &str) -> Result<u32> {
    version
        .parse::<u32>()
        .map_err(|_| StoreError::InvalidInput("invalid type version".into()))
}

fn normalize_version(version: u32, def: &TypeVersion) -> Result<TypeVersionSpec> {
    let mut fields = HashMap::new();
    for (tag_str, field_def) in def.fields.iter() {
        let tag: u64 = tag_str
            .parse()
            .map_err(|_| StoreError::InvalidInput("invalid field tag".into()))?;

        // Parse items spec - can be a simple string or an object with type/ref
        let items = match &field_def.items {
            Some(serde_json::Value::String(s)) => Some(ItemsSpec::Simple(s.clone())),
            Some(serde_json::Value::Object(obj)) => {
                // Check for { "type": "ref", "ref": "cxdb:SomeType" }
                if let Some(serde_json::Value::String(t)) = obj.get("type") {
                    if t == "ref" {
                        if let Some(serde_json::Value::String(r)) = obj.get("ref") {
                            Some(ItemsSpec::Ref(r.clone()))
                        } else {
                            None
                        }
                    } else {
                        Some(ItemsSpec::Simple(t.clone()))
                    }
                // Support shorthand: { "ref": "cxdb.SomeType" } without "type" key
                } else if let Some(serde_json::Value::String(r)) = obj.get("ref") {
                    Some(ItemsSpec::Ref(r.clone()))
                } else {
                    None
                }
            }
            _ => None,
        };

        fields.insert(
            tag,
            FieldSpec {
                name: field_def.name.clone(),
                field_type: field_def.field_type.clone(),
                enum_ref: field_def.enum_ref.clone(),
                type_ref: field_def.type_ref.clone(),
                optional: field_def.optional.unwrap_or(false),
                items,
            },
        );
    }
    Ok(TypeVersionSpec {
        version,
        fields,
        renderer: def.renderer.clone(),
    })
}

fn bundle_filename(bundle_id: &str) -> String {
    let mut safe = bundle_id.replace('/', "_");
    safe = safe.replace(':', "_");
    safe = safe.replace('#', "_");
    format!("bundle_{safe}.json")
}
