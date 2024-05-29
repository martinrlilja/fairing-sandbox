use anyhow::anyhow;

/// Git pkt-line: https://git-scm.com/docs/protocol-common/en#_pkt_line_format
#[derive(Copy, Clone, Debug)]
pub enum PktLine<D> {
    Data(D),
    Flush,
}

#[derive(Copy, Clone, Debug)]
pub struct RefPkt<'a> {
    pub hash: &'a str,
    pub ref_name: &'a str,
    pub capabilities: &'a str,
}

pub fn data_pkt(input: &[u8]) -> nom::IResult<&[u8], &[u8]> {
    let (input, len) = nom::combinator::map_res(
        nom::bytes::streaming::take_while_m_n(4, 4, nom::character::is_hex_digit),
        |s| u16::from_str_radix(std::str::from_utf8(s).unwrap(), 16),
    )(input)?;

    if len <= 4 {
        // TODO: this should probably point to the start of the length.
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Eof,
        )));
    }

    let (input, data) = nom::bytes::streaming::take(len - 4)(input)?;

    Ok((input, data))
}

pub fn flush_pkt<D>(input: &[u8]) -> nom::IResult<&[u8], PktLine<D>> {
    let (input, _) = nom::bytes::streaming::tag(b"0000")(input)?;
    Ok((input, PktLine::Flush))
}

pub fn ref_pkt_line(input: &[u8]) -> nom::IResult<&[u8], PktLine<RefPkt>> {
    nom::branch::alt((flush_pkt, ref_pkt))(input)
}

pub fn ref_pkt(input: &[u8]) -> nom::IResult<&[u8], PktLine<RefPkt>> {
    let (input, pkt) = data_pkt(input)?;

    // Read hash
    let (pkt, hash) =
        nom::bytes::complete::take_while_m_n(40, 40, nom::character::is_hex_digit)(pkt)?;

    let hash = std::str::from_utf8(hash)
        .map_err(|_| nom::Err::Error(nom::error::Error::new(pkt, nom::error::ErrorKind::Char)))?;

    let (pkt, _) = nom::bytes::complete::tag(" ")(pkt)?;

    // Read ref_name
    let (pkt, ref_name) =
        nom::bytes::complete::take_while_m_n(1, 128, |c: u8| c != b'\0' && c != b'\n')(pkt)?;

    let ref_name = std::str::from_utf8(ref_name)
        .map_err(|_| nom::Err::Error(nom::error::Error::new(pkt, nom::error::ErrorKind::Char)))?;

    // Read capabilities
    let (pkt, capabilities) =
        nom::bytes::complete::take_while_m_n(0, 16_384, |c: u8| c != b'\n')(pkt)?;

    let capabilities = std::str::from_utf8(capabilities)
        .map_err(|_| nom::Err::Error(nom::error::Error::new(pkt, nom::error::ErrorKind::Char)))?;

    Ok((
        input,
        PktLine::Data(RefPkt {
            hash,
            ref_name,
            capabilities,
        }),
    ))
}

#[derive(Copy, Clone, Debug)]
pub struct PackFileHeader {
    pub version: u32,
    pub objects: u32,
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum PackFileObjectType {
    Commit,
    Tree,
    Blob,
    Tag,
    RefDelta { parent: [u8; 20] },
}

#[derive(Copy, Clone, Debug)]
pub struct PackFileObjectHeader {
    pub type_: PackFileObjectType,
    pub length: u64,
}

pub fn pack_file_header(input: &[u8]) -> nom::IResult<&[u8], PackFileHeader> {
    let (input, _) = nom::bytes::streaming::tag(b"PACK")(input)?;
    let (input, version) = nom::number::streaming::be_u32(input)?;
    let (input, objects) = nom::number::streaming::be_u32(input)?;

    let header = PackFileHeader { version, objects };

    Ok((input, header))
}

pub fn pack_file_object_header(input: &[u8]) -> nom::IResult<&[u8], PackFileObjectHeader> {
    let input_start = input;

    let (input, (has_long_object_length, object_type, object_length_first)) =
        nom::bits::bits::<_, (bool, u8, u64), nom::error::Error<(&[u8], usize)>, _, _>(
            nom::sequence::tuple((
                nom::combinator::map(nom::bits::streaming::take(1_u8), |bit: u8| bit == 1),
                nom::bits::streaming::take(3_u8),
                nom::bits::streaming::take(4_u8),
            )),
        )(input)?;

    let (input, object_length) = if has_long_object_length {
        let (input, object_length_rest) = pack_file_variable_length(input)?;

        (input, (object_length_rest << 4) | object_length_first)
    } else {
        (input, object_length_first)
    };

    let (input, object_type) = match object_type {
        1 => (input, PackFileObjectType::Commit),
        2 => (input, PackFileObjectType::Tree),
        3 => (input, PackFileObjectType::Blob),
        4 => (input, PackFileObjectType::Tag),
        7 => {
            let (input, parent) = nom::bytes::streaming::take(20usize)(input)?;
            let parent = {
                let mut output = [0u8; 20];
                output.copy_from_slice(parent);
                output
            };

            (input, PackFileObjectType::RefDelta { parent })
        }
        _ => {
            return Err(nom::Err::Failure(nom::error::Error::new(
                input_start,
                nom::error::ErrorKind::Verify,
            )))
        }
    };

    Ok((
        input,
        PackFileObjectHeader {
            type_: object_type,
            length: object_length,
        },
    ))
}

pub fn pack_file_variable_length(input: &[u8]) -> nom::IResult<&[u8], u64> {
    let (input, ((offset, object_length_first), object_length_last)) =
        nom::bits::<_, ((u64, u64), u64), nom::error::Error<(&[u8], usize)>, _, _>(
            nom::sequence::tuple((
                nom::multi::fold_many0(
                    nom::sequence::preceded(
                        nom::bits::streaming::tag(1_u8, 1_u8),
                        nom::bits::streaming::take(7_u8),
                    ),
                    || (0, 0),
                    |(offset, size): (u64, u64), value: u64| (offset + 7, (value << offset) | size),
                ),
                nom::sequence::preceded(
                    nom::bits::streaming::tag(0_u8, 1_u8),
                    nom::bits::streaming::take(7_u8),
                ),
            )),
        )(input)?;

    Ok((input, (object_length_last << offset) | object_length_first))
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum DeltaInstruction<'a> {
    CopyFromParent { offset: u64, length: u64 },
    InsertData(&'a [u8]),
}

pub fn delta_instruction<'a>(input: &'a [u8]) -> nom::IResult<&[u8], DeltaInstruction<'a>> {
    nom::bits::<_, _, nom::error::Error<(&[u8], usize)>, _, _>(nom::branch::alt((
        nom::sequence::preceded(
            nom::bits::streaming::tag(1u8, 1u8),
            nom::combinator::flat_map(nom::bits::streaming::take(7u8), |v: u8| {
                let bytes_length = v.count_ones();

                nom::combinator::map(
                    nom::bits::streaming::take(bytes_length * 8),
                    move |mut b: u64| {
                        let mut length = 0;
                        for i in (0..3).rev() {
                            if (v >> (i + 4)) & 1 == 1 {
                                length |= (b & 0xff) << (i * 8);
                                b >>= 8;
                            }
                        }

                        if length == 0 {
                            length = 0x10000;
                        }

                        let mut offset = 0;
                        for i in (0..4).rev() {
                            if (v >> i) & 1 == 1 {
                                offset |= (b & 0xff) << (i * 8);
                                b >>= 8;
                            }
                        }

                        DeltaInstruction::CopyFromParent { offset, length }
                    },
                )
            }),
        ),
        nom::sequence::preceded(
            nom::bits::streaming::tag(0u8, 1u8),
            nom::combinator::flat_map(nom::bits::streaming::take(7u8), |length: u64| {
                nom::combinator::map(
                    nom::bytes::<_, _, nom::error::Error<&[u8]>, _, _>(
                        nom::bytes::streaming::take(length),
                    ),
                    |data| DeltaInstruction::InsertData(data),
                )
            }),
        ),
    )))(input)
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Commit {
    pub tree: [u8; 20],
    pub parent: Option<[u8; 20]>,
    //author: CommitPersonDate<'a>,
    //committer: CommitPersonDate<'a>,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CommitPersonDate<'a> {
    pub name: &'a str,
    pub email: &'a str,
    pub timestamp: i64,
    pub timezone: i16,
}

fn object_hash_text_to_binary(input: &[u8]) -> nom::IResult<&[u8], [u8; 20]> {
    nom::combinator::map_res(
        nom::bytes::complete::take_while_m_n(40, 40, nom::character::is_hex_digit),
        |str_hash| {
            let mut hash = [0u8; 20];
            hex::decode_to_slice(str_hash, &mut hash).map(|()| hash)
        },
    )(input)
}

pub fn commit_object<'a>(input: &'a [u8]) -> nom::IResult<&[u8], Commit> {
    let (input, tree) = nom::sequence::delimited(
        nom::bytes::complete::tag(b"tree "),
        object_hash_text_to_binary,
        nom::bytes::complete::tag(b"\n"),
    )(input)?;

    let (input, parent) = nom::combinator::opt(nom::sequence::delimited(
        nom::bytes::complete::tag(b"tree "),
        object_hash_text_to_binary,
        nom::bytes::complete::tag(b"\n"),
    ))(input)?;

    Ok((input, Commit { tree, parent }))
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TreeItem<'a> {
    Blob {
        mode: TreeItemBlobMode,
        hash: [u8; 20],
        name: &'a str,
    },
    Tree {
        hash: [u8; 20],
        name: &'a str,
    },
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TreeItemBlobMode {
    Normal,
    Executable,
    SymbolicLink,
}

fn object_hash_binary(input: &[u8]) -> nom::IResult<&[u8], [u8; 20]> {
    nom::combinator::map(nom::bytes::complete::take(20_usize), |hash| {
        let mut out_hash = [0u8; 20];
        out_hash.copy_from_slice(hash);
        out_hash
    })(input)
}

pub fn tree_item<'a>(input: &'a [u8]) -> nom::IResult<&[u8], TreeItem<'a>> {
    nom::branch::alt((
        nom::combinator::map_res(
            nom::sequence::tuple((
                tree_item_blob_mode,
                nom::bytes::complete::tag(b" "),
                nom::bytes::complete::take_while_m_n(1, 200, |c| c != b'\0' && c != b'/'),
                nom::bytes::complete::tag(b"\0"),
                object_hash_binary,
            )),
            |(mode, _, name, _, hash)| {
                Ok::<_, anyhow::Error>(TreeItem::Blob {
                    mode,
                    hash,
                    name: std::str::from_utf8(name).map_err(|_| anyhow!("invalid name"))?,
                })
            },
        ),
        nom::combinator::map_res(
            nom::sequence::tuple((
                nom::bytes::complete::tag(b"40000 "),
                nom::bytes::complete::take_while_m_n(1, 200, |c| c != b'\0' && c != b'/'),
                nom::bytes::complete::tag(b"\0"),
                object_hash_binary,
            )),
            |(_, name, _, hash)| {
                Ok::<_, anyhow::Error>(TreeItem::Tree {
                    hash,
                    name: std::str::from_utf8(name).map_err(|_| anyhow!("invalid name"))?,
                })
            },
        ),
    ))(input)
}

fn tree_item_blob_mode(input: &[u8]) -> nom::IResult<&[u8], TreeItemBlobMode> {
    nom::branch::alt((
        nom::combinator::map(nom::bytes::complete::tag(b"100644"), |_| {
            TreeItemBlobMode::Normal
        }),
        nom::combinator::map(nom::bytes::complete::tag(b"100755"), |_| {
            TreeItemBlobMode::Executable
        }),
        nom::combinator::map(nom::bytes::complete::tag(b"120000"), |_| {
            TreeItemBlobMode::SymbolicLink
        }),
    ))(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_instruction_insert_data() {
        assert_eq!(
            delta_instruction(&[0b000000011, 1, 2, 3]),
            nom::IResult::Ok((&[][..], DeltaInstruction::InsertData(&[1, 2, 3])))
        );
    }

    #[test]
    fn delta_instruction_copy_from_parent() {
        assert_eq!(
            delta_instruction(&[0b10010001, 1, 2]),
            nom::IResult::Ok((
                &[][..],
                DeltaInstruction::CopyFromParent {
                    offset: 1,
                    length: 2,
                }
            ))
        );

        assert_eq!(
            delta_instruction(&[0b10010011, 1, 2, 3]),
            nom::IResult::Ok((
                &[][..],
                DeltaInstruction::CopyFromParent {
                    offset: 513,
                    length: 3,
                }
            ))
        );

        assert_eq!(
            delta_instruction(&[0b11010001, 1, 2, 3]),
            nom::IResult::Ok((
                &[][..],
                DeltaInstruction::CopyFromParent {
                    offset: 1,
                    length: 196610,
                }
            ))
        );
    }
}
