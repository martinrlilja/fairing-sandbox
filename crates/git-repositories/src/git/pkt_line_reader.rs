use fairing_core2::models;

use super::{
    parsers::{ref_pkt_line, PktLine, RefPkt},
    SshClient, SshReader,
};

pub enum GitPktLineOutput {
    RefPkt(models::GitSourceRefAndCommit),
    Flush,
}

pub struct GitPktLineReader {
    head_hash: Option<String>,
    capabilities: Option<String>,
}

impl GitPktLineReader {
    pub fn new() -> GitPktLineReader {
        GitPktLineReader {
            head_hash: None,
            capabilities: None,
        }
    }

    pub fn capabilities(&self) -> Option<&str> {
        self.capabilities.as_ref().map(|s| s.as_str())
    }
}

#[async_trait::async_trait]
impl SshReader for GitPktLineReader {
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
                let hash = hash.to_owned();

                match self.head_hash {
                    Some(ref head_hash) if head_hash == &hash => (),
                    _ => return Ok((input, None)),
                }

                let build = models::GitSourceRefAndCommit {
                    ref_: ref_name.into(),
                    commit: hash,
                };

                Ok((input, Some(GitPktLineOutput::RefPkt(build))))
            }
            PktLine::Data(RefPkt { .. }) => {
                // Ignore anything that is not a branch.
                Ok((input, None))
            }
            PktLine::Flush => Ok((input, Some(GitPktLineOutput::Flush))),
        }
    }
}
