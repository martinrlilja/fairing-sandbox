use anyhow::{anyhow, Result};
use std::{collections::BTreeMap, str::FromStr};
use uuid::Uuid;

use super::{uuid_v7, FileChecksum, ProjectId, Source, SourceName, WorkerId};

#[derive(Clone, Debug, PartialEq)]
pub struct LayerSetName(String);

impl LayerSetName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for LayerSetName {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<LayerSetName> {
        Ok(LayerSetName(s.into()))
    }
}

#[derive(Clone, Debug)]
pub struct LayerSet {
    pub project_id: ProjectId,
    pub name: LayerSetName,

    pub visibility: LayerSetVisibility,

    pub source: Option<LayerSetSource>,

    pub build_status: LayerSetBuildStatus,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum LayerSetVisibility {
    Private,
    Public,
}

#[derive(Clone, Debug)]
pub struct LayerSetSource {
    pub name: SourceName,
    pub kind: LayerSetSourceKind,
}

#[derive(Clone, Debug)]
pub enum LayerSetSourceKind {
    Git { ref_: String },
}

#[derive(Clone, Debug)]
pub struct LayerSetBuildStatus {
    pub current_layer_id: Option<LayerId>,
    pub last_layer_id: Option<LayerId>,
}

#[derive(Clone, Debug)]
pub struct CreateLayerSet {
    pub name: LayerSetName,
    pub visibility: LayerSetVisibility,
    pub source: Option<CreateLayerSetSource>,
}

#[derive(Clone, Debug)]
pub struct CreateLayerSetSource {
    pub source: Source,
    pub kind: CreateLayerSetSourceKind,
}

#[derive(Clone, Debug)]
pub enum CreateLayerSetSourceKind {
    Git { ref_: String },
}

#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct LayerId(Uuid);

impl LayerId {
    pub fn new() -> Result<LayerId> {
        Ok(LayerId(uuid_v7()?))
    }

    pub fn into_uuid(self) -> Uuid {
        let LayerId(uuid) = self;
        uuid
    }
}

impl From<Uuid> for LayerId {
    fn from(uuid: Uuid) -> LayerId {
        LayerId(uuid)
    }
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub project_id: ProjectId,
    pub layer_set_name: LayerSetName,
    pub id: LayerId,
    pub status: LayerStatus,
    pub source: Option<LayerSource>,
}

#[derive(Copy, Clone, Debug)]
pub enum LayerStatus {
    Building,
    Finalizing,
    Ready,
    Cancelled,
}

#[derive(Clone, Debug)]
pub enum LayerSource {
    Git { commit: String },
}

#[derive(Clone, Debug)]
pub struct CreateLayer {
    pub source: Option<CreateLayerSource>,
}

#[derive(Clone, Debug)]
pub enum CreateLayerSource {
    Git { commit: String },
}

#[derive(Clone, Debug)]
pub struct LayerChange {
    pub project_id: ProjectId,
    pub layer_set_name: LayerSetName,
    pub layer_id: LayerId,
    pub worker_id: WorkerId,

    pub path: String,
    pub checksum: FileChecksum,

    pub content_encoding_hint: ContentEncodingHint,
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct LayerMember {
    pub project_id: ProjectId,
    pub layer_set_name: LayerSetName,
    pub layer_id: LayerId,

    pub path: String,
    pub checksum: FileChecksum,
    pub content_encoding_hint: ContentEncodingHint,
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct LayerMemberSummary {
    pub path: String,
    pub checksum: FileChecksum,
    pub content_encoding_hint: ContentEncodingHint,
    pub headers: BTreeMap<String, String>,
}

#[derive(Copy, Clone, Debug, bincode::Encode, bincode::Decode)]
pub enum ContentEncodingHint {
    Relative {
        identity: u8,
        gzip: u8,
        zstd: u8,
        brotli: u8,
    },
}

impl ContentEncodingHint {
    pub fn encode(&self) -> [u8; 8] {
        let config = bincode::config::standard().skip_fixed_array_length();
        let mut data = [0u8; 8];
        bincode::encode_into_slice(self, &mut data, config).unwrap();
        data
    }

    pub fn decode(bytes: &[u8]) -> Result<ContentEncodingHint> {
        let config = bincode::config::standard().skip_fixed_array_length();
        let (content_encoding_hint, _) = bincode::decode_from_slice(bytes, config)?;
        Ok(content_encoding_hint)
    }
}

/*
impl ContentEncodingHint {
    pub fn encode(&self) -> u64 {
        match self {
            ContentEncodingHint::Relative { identity, gzip, zstd, brotli } => {
                ((brotli & 0xf) as u64) << 16
                    | ((zstd & 0xf) as u64) << 12
                    | ((gzip & 0xf) as u64) << 8
                    | ((identity & 0xf) as u64) << 4
            }
        }
    }

    pub fn decode(value: u64) -> Result<ContentEncodingHint> {
        let kind = value & 0xf;
        match kind {
            0 => {
                let identity = ((value >> 4) & 0xf) as u8;
                let gzip = ((value >> 8) & 0xf) as u8;
                let zstd = ((value >> 12) & 0xf) as u8;
                let brotli = ((value >> 16) & 0xf) as u8;

                Ok(ContentEncodingHint::Relative {
                    identity,
                    gzip,
                    zstd,
                    brotli,
                })
            }
            _ => Err(anyhow!("unknown content encoding hint kind: {kind:x}")),
        }
    }
}
*/

/*
#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
pub enum Content {
    Deleted,
    WithEncoding {
        identity: ContentWithEncoding,
        gzip: ContentWithEncoding,
        zstd: ContentWithEncoding,
        brotli: ContentWithEncoding,
    },
}

#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
pub enum ContentWithEncoding {
    None,
    WithSize {
        checksum: FileChecksum,
        size: u64,
    }
}

impl ContentEncodingHint {
    pub fn encode(&self) -> Vec<u8> {
        let config = bincode::config::standard().skip_fixed_array_length();
        bincode::encode_to_vec(self, config).unwrap()
    }

    pub fn decode(bytes: &[u8]) -> Result<ContentEncodingHint> {
        let config = bincode::config::standard().skip_fixed_array_length();
        let (checksum, _) = bincode::decode_from_slice(bytes, config)?;
        Ok(checksum)
    }
}
*/
