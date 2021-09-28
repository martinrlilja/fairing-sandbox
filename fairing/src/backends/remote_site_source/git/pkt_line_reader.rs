use std::borrow::Cow;

use fairing_core::models::{self, prelude::*};

use super::{
    parsers::{ref_pkt_line, PktLine, RefPkt},
    SshClient, SshReader,
};

pub enum GitPktLineOutput {
    RefPkt(models::CreateTreeRevision<'static>),
    Flush,
}

pub struct GitPktLineReader<'n> {
    site_source_name: &'n models::SiteSourceName<'n>,
    head_hash: Option<String>,
    capabilities: Option<String>,
}

impl<'n> GitPktLineReader<'n> {
    pub fn new(site_source_name: &'n models::SiteSourceName<'n>) -> GitPktLineReader<'n> {
        GitPktLineReader {
            site_source_name,
            head_hash: None,
            capabilities: None,
        }
    }

    pub fn capabilities(&self) -> Option<&str> {
        self.capabilities.as_ref().map(|s| s.as_str())
    }
}

#[async_trait::async_trait]
impl<'n> SshReader for GitPktLineReader<'n> {
    type Output = Option<GitPktLineOutput>;

    async fn read<'a>(
        &mut self,
        _client: &mut SshClient,
        input: &'a [u8],
    ) -> nom::IResult<&'a [u8], Self::Output> {
        let (input, pkt_line) = ref_pkt_line(input)?;

        // Only the first pkt line is allowed to contain capabilities.
        if self.capabilities.is_none() {
            if let PktLine::Data(RefPkt { capabilities, .. }) = pkt_line {
                self.capabilities = Some(capabilities.to_owned());
            }
        }

        match pkt_line {
            PktLine::Data(RefPkt {
                hash,
                ref_name: "HEAD",
                ..
            }) => {
                self.head_hash = Some(hash.to_owned());
                Ok((input, None))
            }
            PktLine::Data(RefPkt { hash, ref_name, .. }) if ref_name.starts_with("refs/heads/") => {
                let ref_name = ref_name.replace('/', ":");
                let hash = hash.to_owned();

                let status = match self.head_hash {
                    Some(ref head_hash) if head_hash == &hash => models::TreeRevisionStatus::Fetch,
                    _ => models::TreeRevisionStatus::Ignore,
                };

                let tree_name = format!("{}/trees/{}", self.site_source_name.name(), ref_name);
                let revision = models::CreateTreeRevision {
                    resource_id: Cow::Owned(hash),
                    parent: models::TreeName::parse(tree_name).unwrap(),
                    status,
                };

                Ok((input, Some(GitPktLineOutput::RefPkt(revision))))
            }
            PktLine::Data(RefPkt { .. }) => {
                // Ignore anything that is not a branch.
                Ok((input, None))
            }
            PktLine::Flush => Ok((input, Some(GitPktLineOutput::Flush))),
        }
    }
}
