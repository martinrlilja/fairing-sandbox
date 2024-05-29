use uuid::Uuid;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ProjectId(Uuid);

impl ProjectId {
    pub fn into_uuid(self) -> Uuid {
        let ProjectId(uuid) = self;
        uuid
    }
}

impl From<Uuid> for ProjectId {
    fn from(uuid: Uuid) -> ProjectId {
        ProjectId(uuid)
    }
}

impl Into<Uuid> for ProjectId {
    fn into(self) -> Uuid {
        let ProjectId(uuid) = self;
        uuid
    }
}

impl bincode::Encode for ProjectId {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> core::result::Result<(), bincode::error::EncodeError> {
        bincode::Encode::encode(&self.0.as_u128(), encoder)?;
        Ok(())
    }
}

impl bincode::Decode for ProjectId {
    fn decode<D: bincode::de::Decoder>(
        decoder: &mut D,
    ) -> core::result::Result<Self, bincode::error::DecodeError> {
        Ok(Self(Uuid::from_u128(bincode::Decode::decode(decoder)?)))
    }
}

impl<'de> bincode::BorrowDecode<'de> for ProjectId {
    fn borrow_decode<D: bincode::de::BorrowDecoder<'de>>(
        decoder: &mut D,
    ) -> core::result::Result<Self, bincode::error::DecodeError> {
        Ok(Self(Uuid::from_u128(bincode::Decode::decode(decoder)?)))
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    pub id: ProjectId,
    pub acme_dns_challenge_label: String,
    pub file_encryption_key: Vec<u8>,
}

#[derive(Copy, Clone, Debug)]
pub struct CreateProject;
